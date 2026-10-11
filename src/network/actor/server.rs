use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use super::{Actor, CONN_CONTROL_QUEUE_CAPACITY};
use crate::network::NetworkEvent;
use crate::network::conn::{ConnControl, run_server_conn};
use crate::protocol::{
    LOGIN_MINOR_VERSION, LOGIN_VERSION, LoginOutcome, ServerRequest, ServerResponse,
};
use crate::types::{ConnectionType, UserStatus};

const LOGIN_TIMEOUT: Duration = Duration::from_secs(30);
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_secs(1);

pub(super) struct ServerState {
    control: mpsc::Sender<ConnControl>,
    username: String,
    logged_in: bool,
    login_deadline: Option<Instant>,
    manual_disconnect: bool,
}

pub(super) struct Server {
    state: Option<ServerState>,
    listen_port: u16,
    pub(super) listener: Option<TcpListener>,
    pub(super) accept_resumes_at: Option<Instant>,
}

impl Server {
    pub(super) fn new() -> Self {
        Self {
            state: None,
            listen_port: 0,
            listener: None,
            accept_resumes_at: None,
        }
    }

    pub(super) fn username(&self) -> Option<&str> {
        self.state.as_ref().map(|state| state.username.as_str())
    }

    pub(super) fn is_logged_in(&self) -> bool {
        self.state.as_ref().is_some_and(|state| state.logged_in)
    }

    pub(super) fn mark_logged_in(&mut self) -> Option<String> {
        let state = self.state.as_mut()?;
        state.logged_in = true;
        Some(state.username.clone())
    }

    pub(super) fn mark_manual_disconnect(&mut self) {
        if let Some(state) = &mut self.state {
            state.manual_disconnect = true;
        }
    }

    pub(super) fn teardown(&mut self) {
        self.listener = None;
        if let Some(state) = self.state.take() {
            let _ = state.control.try_send(ConnControl::Close);
        }
    }
}

pub(super) async fn accept(
    listener: &Option<TcpListener>,
    resumes_at: Option<Instant>,
) -> std::io::Result<(TcpStream, SocketAddr)> {
    let Some(listener) = listener else {
        return std::future::pending().await;
    };
    if let Some(resumes_at) = resumes_at {
        tokio::time::sleep_until(resumes_at).await;
    }
    listener.accept().await
}

impl Actor {
    pub(super) fn server_connect(
        &mut self,
        address: SocketAddr,
        username: String,
        password: String,
        listen_port: u16,
    ) {
        if self.server.state.is_some() {
            warn!("already connected to server, ignoring connect request");
            return;
        }
        let listener = match std::net::TcpListener::bind(("0.0.0.0", listen_port)) {
            Ok(listener) => listener,
            Err(error) => {
                warn!(%error, listen_port, "cannot bind listen port");
                self.emit(NetworkEvent::ListenFailed {
                    port: listen_port,
                    error: error.to_string(),
                });
                return;
            }
        };
        listener
            .set_nonblocking(true)
            .expect("set listener nonblocking");
        let listener =
            TcpListener::from_std(listener).expect("register listener with tokio runtime");
        self.server.listen_port = listen_port;
        let (control_tx, control_rx) = mpsc::channel(CONN_CONTROL_QUEUE_CAPACITY);
        super::spawn_conn_task(
            "server connection",
            run_server_conn(address, self.conn_events_tx.clone(), control_rx),
        );

        let login = ServerRequest::Login {
            username: username.clone(),
            password,
            version: LOGIN_VERSION,
            minor_version: LOGIN_MINOR_VERSION,
        };
        control_tx
            .try_send(ConnControl::Send(login.to_bytes()))
            .expect("fresh server control queue rejected send");
        control_tx
            .try_send(ConnControl::Send(
                ServerRequest::SetWaitPort {
                    port: listen_port as u32,
                }
                .to_bytes(),
            ))
            .expect("fresh server control queue rejected send");

        self.distributed.branch_root = Some(username.clone());
        self.indirect.cache_address(
            username.clone(),
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, listen_port),
        );
        self.server.state = Some(ServerState {
            control: control_tx,
            username,
            logged_in: false,
            login_deadline: Some(Instant::now() + LOGIN_TIMEOUT),
            manual_disconnect: false,
        });

        info!(listen_port, "listening for peer connections");
        self.server.listener = Some(listener);
        self.server.accept_resumes_at = None;
    }

    pub(super) fn handle_accept(&mut self, accepted: std::io::Result<(TcpStream, SocketAddr)>) {
        match accepted {
            Ok((stream, addr)) => {
                self.server.accept_resumes_at = None;
                self.handle_accepted(stream, addr);
            }
            Err(error) => {
                warn!(%error, "accepting peer connection failed, backing off");
                self.server.accept_resumes_at = Some(Instant::now() + ACCEPT_ERROR_BACKOFF);
            }
        }
    }

    pub(super) fn check_login_timeout(&mut self) {
        let Some(state) = &mut self.server.state else {
            return;
        };
        if state.logged_in
            || state
                .login_deadline
                .is_none_or(|deadline| Instant::now() < deadline)
        {
            return;
        }
        state.login_deadline = None;
        warn!("server did not answer login, disconnecting");
        self.push_server(ConnControl::Close);
    }

    pub(super) fn push_server(&mut self, control: ConnControl) {
        let Some(state) = &self.server.state else {
            return;
        };
        let overflowed = matches!(
            state.control.try_send(control),
            Err(mpsc::error::TrySendError::Full(_))
        );
        if overflowed {
            warn!("server outbound queue overflowed, dropping connection");
            self.handle_server_closed(Some("outbound queue overflowed".into()));
        }
    }

    pub(super) fn send_to_server(&mut self, request: ServerRequest) {
        if self.server.state.is_none() {
            debug!(?request, "dropping server message, not connected");
            return;
        }
        self.push_server(ConnControl::Send(request.to_bytes()));
    }

    pub(super) fn handle_server_message(&mut self, message: ServerResponse) {
        match &message {
            ServerResponse::Login(outcome) => match outcome {
                LoginOutcome::Success { banner, .. } => {
                    let Some(username) = self.server.mark_logged_in() else {
                        return;
                    };
                    self.emit(NetworkEvent::LoggedIn {
                        username,
                        banner: banner.clone(),
                    });
                    self.send_have_no_parent();
                }
                LoginOutcome::Failure { reason, detail } => {
                    self.emit(NetworkEvent::LoginRejected {
                        reason: reason.clone(),
                        detail: detail.clone(),
                    });
                    self.server.mark_manual_disconnect();
                    self.push_server(ConnControl::Close);
                }
            },
            ServerResponse::ConnectToPeer {
                user,
                conn_type,
                ip_address,
                port,
                token,
                ..
            } => match (
                ConnectionType::from_str_value(conn_type),
                u16::try_from(*port).ok().filter(|&port| port != 0),
            ) {
                (Some(conn_type), Some(port)) => {
                    let addr = SocketAddrV4::new(*ip_address, port);
                    self.handle_connect_to_peer(user.clone(), conn_type, addr, *token);
                }
                (None, _) => debug!(conn_type, "unknown connection type"),
                (_, None) => debug!(user, port, "skipping indirect connection, invalid port"),
            },
            ServerResponse::CantConnectToPeer { token } => {
                self.handle_cant_connect(*token);
            }
            ServerResponse::GetUserStatus { user, status, .. } => {
                if *status == UserStatus::Offline.as_u32() {
                    self.indirect.forget_address(user);
                }
            }
            ServerResponse::GetPeerAddress {
                user,
                ip_address,
                port,
                ..
            } => {
                self.handle_peer_address(user, *ip_address, *port);
            }
            ServerResponse::WatchUser {
                user,
                user_exists,
                stats,
                ..
            } => {
                if Some(user.as_str()) == self.server.username() {
                    if let Some(stats) = stats {
                        self.update_own_speed(stats.avgspeed);
                    }
                } else if !user_exists {
                    self.indirect.unwatch_user(user);
                }
            }
            ServerResponse::GetUserStats { user, stats } => {
                if Some(user.as_str()) == self.server.username() {
                    self.update_own_speed(stats.avgspeed);
                }
            }
            ServerResponse::Relogged => {
                self.server.mark_manual_disconnect();
            }
            ServerResponse::PossibleParents { parents } => {
                self.handle_possible_parents(parents);
            }
            ServerResponse::ParentMinSpeed { speed } => {
                self.distributed.parent_min_speed = *speed;
                self.update_max_distrib_children();
            }
            ServerResponse::ParentSpeedRatio { ratio } => {
                self.distributed.parent_speed_ratio = *ratio;
                self.update_max_distrib_children();
            }
            ServerResponse::ResetDistributed => {
                self.handle_reset_distributed();
            }
            ServerResponse::EmbeddedMessage {
                distrib_code,
                distrib_message,
            } => {
                self.handle_embedded_message(*distrib_code, distrib_message);
                return;
            }
            _ => {}
        }
        self.emit(NetworkEvent::ServerMessage(message));
    }

    pub(super) fn handle_server_closed(&mut self, error: Option<String>) {
        let Some(state) = self.server.state.take() else {
            return;
        };
        if let Some(error) = error {
            warn!(%error, "server connection closed");
        }
        self.server.listener = None;
        self.peers.close_all();
        self.indirect.reset();
        self.distributed.reset();
        {
            let mut allowed = self.allowed.write().unwrap();
            allowed.search_tokens.clear();
            allowed.shared_list_users.clear();
            allowed.user_info_users.clear();
            allowed.folder_contents.clear();
        }
        self.emit(NetworkEvent::ServerDisconnected {
            manual: state.manual_disconnect,
        });
    }
}

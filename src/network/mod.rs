mod actor;
mod api;
mod codec;
mod conn;

use tokio::sync::mpsc;

pub use api::{ConnId, NetworkCommand, NetworkEvent};

pub const COMMAND_QUEUE_CAPACITY: usize = 1024;
pub const EVENT_QUEUE_CAPACITY: usize = 4096;

#[derive(Clone)]
pub struct NetworkHandle {
    commands: mpsc::Sender<NetworkCommand>,
}

impl NetworkHandle {
    pub fn send(&self, command: NetworkCommand) {
        match self.commands.try_send(command) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                panic!("network actor command queue overflowed")
            }
            Err(mpsc::error::TrySendError::Closed(_)) => panic!("network actor terminated"),
        }
    }

    pub fn server(&self, request: crate::protocol::ServerRequest) {
        self.send(NetworkCommand::SendServerMessage(request));
    }

    pub fn peer(&self, username: impl Into<String>, message: crate::protocol::PeerMessage) {
        self.send(NetworkCommand::SendPeerMessage {
            username: username.into(),
            message,
        });
    }

    pub fn peer_frame(&self, username: impl Into<String>, bytes: Vec<u8>) {
        self.send(NetworkCommand::SendPeerFrame {
            username: username.into(),
            bytes,
        });
    }
}

#[cfg(test)]
pub(crate) fn test_channel() -> (NetworkHandle, mpsc::Receiver<NetworkCommand>) {
    let (commands, rx) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
    (NetworkHandle { commands }, rx)
}

pub fn spawn() -> (NetworkHandle, mpsc::Receiver<NetworkEvent>) {
    let (commands_tx, commands_rx) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
    let (events_tx, events_rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);
    tokio::spawn(actor::Actor::new(commands_rx, events_tx).run());
    (
        NetworkHandle {
            commands: commands_tx,
        },
        events_rx,
    )
}

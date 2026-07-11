mod actor;
mod codec;
mod command;
mod conn;
mod event;

use tokio::sync::mpsc;

pub use command::NetworkCommand;
pub use event::{ConnId, NetworkEvent};

#[derive(Clone)]
pub struct NetworkHandle {
    commands: mpsc::UnboundedSender<NetworkCommand>,
}

impl NetworkHandle {
    pub fn send(&self, command: NetworkCommand) {
        let _ = self.commands.send(command);
    }

    pub fn server(&self, request: crate::protocol::ServerRequest) {
        self.send(NetworkCommand::SendServerMessage(request));
    }

    pub fn peer(&self, username: impl Into<String>, message: crate::protocol::PeerMessage) {
        self.send(NetworkCommand::SendPeerMessage { username: username.into(), message });
    }
}

pub fn spawn() -> (NetworkHandle, mpsc::UnboundedReceiver<NetworkEvent>) {
    let (commands_tx, commands_rx) = mpsc::unbounded_channel();
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    tokio::spawn(actor::Actor::new(commands_rx, events_tx).run());
    (NetworkHandle { commands: commands_tx }, events_rx)
}

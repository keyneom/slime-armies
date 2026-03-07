use crate::webrtc_socket::{PeerId, PeerRequest, PeerSignal, SignalingRequest};
use futures_channel::mpsc::UnboundedSender;

#[derive(Debug, Clone)]
pub struct SignalPeer {
    pub id: PeerId,
    pub sender: UnboundedSender<SignalingRequest>,
}

impl SignalPeer {
    pub fn send(&self, signal: PeerSignal) {
        let req = PeerRequest::Signal {
            receiver: self.id,
            data: signal,
        };
        self.sender
            .unbounded_send(SignalingRequest::Peer(req))
            .expect("Send error");
    }

    pub fn new(id: PeerId, sender: UnboundedSender<SignalingRequest>) -> Self {
        Self { id, sender }
    }
}

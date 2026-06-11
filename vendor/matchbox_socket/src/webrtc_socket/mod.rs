pub(crate) mod error;
mod messages;
mod signal_peer;
mod socket;

use self::error::SignalingError;
use crate::{webrtc_socket::signal_peer::SignalPeer, Error};
use async_trait::async_trait;
use cfg_if::cfg_if;
use futures::{future::Either, stream::FuturesUnordered, Future, FutureExt, StreamExt};
use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures_timer::Delay;
use futures_util::select;
use log::{debug, error, warn};
use matchbox_protocol::PeerId;
use messages::*;
pub(crate) use socket::MessageLoopChannels;
pub use socket::{
    BuildablePlurality, ChannelConfig, ChannelPlurality, MultipleChannels, NoChannels, PeerState,
    RtcIceServerConfig, SingleChannel, WebRtcChannel, WebRtcSocket, WebRtcSocketBuilder,
};
use std::{
    collections::{HashMap, HashSet},
    pin::Pin,
    time::Duration,
};

cfg_if! {
    if #[cfg(target_arch = "wasm32")] {
        mod wasm;
        type UseMessenger = wasm::WasmMessenger;
        type UseSignaller = wasm::WasmSignaller;
        /// A future which runs the message loop for the socket and completes
        /// when the socket closes or disconnects
        pub type MessageLoopFuture = Pin<Box<dyn Future<Output = Result<(), Error>>>>;
    } else {
        mod native;
type UseMessenger = native::NativeMessenger;
        type UseSignaller = native::NativeSignaller;
        /// A future which runs the message loop for the socket and completes
        /// when the socket closes or disconnects
        pub type MessageLoopFuture = Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>;
    }
}

#[derive(Debug)]
pub(crate) enum SignalingRequest {
    Peer(PeerRequest),
    Disconnect,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
trait Signaller: Sized {
    async fn new(mut attempts: Option<u16>, room_url: &str) -> Result<Self, SignalingError>;

    async fn send(&mut self, request: String) -> Result<(), SignalingError>;

    async fn next_message(&mut self) -> Result<String, SignalingError>;
}

async fn signaling_loop<S: Signaller>(
    attempts: Option<u16>,
    room_url: String,
    mut requests_receiver: futures_channel::mpsc::UnboundedReceiver<SignalingRequest>,
    events_sender: futures_channel::mpsc::UnboundedSender<PeerEvent>,
) -> Result<(), SignalingError> {
    let mut signaller = S::new(attempts, &room_url).await?;

    loop {
        select! {
            request = requests_receiver.next().fuse() => {
                match request {
                    Some(SignalingRequest::Peer(request)) => {
                        let request = serde_json::to_string(&request).expect("serializing request");
                        debug!("-> {request}");
                        signaller.send(request).await.map_err(SignalingError::from)?;
                    }
                    Some(SignalingRequest::Disconnect) | None => {
                        break Ok(());
                    }
                }
            }

            message = signaller.next_message().fuse() => {
                match message {
                    Ok(message) => {
                        debug!("Received {message}");
                        let event: PeerEvent = serde_json::from_str(&message)
                            .unwrap_or_else(|err| panic!("couldn't parse peer event: {err}.\nEvent: {message}"));
                        events_sender.unbounded_send(event).map_err(SignalingError::from)?;
                    }
                    Err(SignalingError::UnknownFormat) => {
                        warn!("ignoring unexpected non-text message from signaling server")
                    },
                    Err(err) => break Err(err)
                }

            }

            complete => break Ok(())
        }
    }
}

/// The raw format of data being sent and received.
pub type Packet = Box<[u8]>;

/// Errors that can happen when sending packets
#[derive(Debug, thiserror::Error)]
#[error("The socket was dropped and package could not be sent")]
struct PacketSendError {
    #[cfg(not(target_arch = "wasm32"))]
    source: futures_channel::mpsc::SendError,
    #[cfg(target_arch = "wasm32")]
    source: error::JsError,
}

trait PeerDataSender {
    fn send(&mut self, packet: Packet) -> Result<(), PacketSendError>;
    fn close(&mut self) {}
}

/// Monotonic-enough wall clock in milliseconds. `std::time::Instant` panics
/// with "time not implemented on this platform" on wasm32-unknown-unknown,
/// and the panic poisons the async executor — every handshake retry after a
/// timeout used to kill networking for the rest of the session.
#[cfg(target_arch = "wasm32")]
fn now_ms() -> f64 {
    js_sys::Date::now()
}

#[cfg(not(target_arch = "wasm32"))]
fn now_ms() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

#[derive(Debug, Clone)]
pub(crate) enum SocketControl {
    FullMesh,
    SetDesiredPeers(HashSet<PeerId>),
    DropPeer(PeerId),
    DetachSignaling,
}

struct HandshakeResult<D: PeerDataSender, M> {
    peer_id: PeerId,
    data_channels: Vec<D>,
    metadata: M,
    established: bool,
}

enum HandshakeOutcome<D: PeerDataSender, M> {
    Result(HandshakeResult<D, M>),
    TimedOut(PeerId),
}

async fn with_handshake_timeout<F, D, M>(peer_id: PeerId, future: F) -> HandshakeOutcome<D, M>
where
    F: Future<Output = HandshakeResult<D, M>>,
    D: PeerDataSender,
{
    // Worst case before the answer even arrives: offerer ICE-gathering cap
    // (3s) + signaling latency + answerer ICE-gathering cap (3s) + data
    // channel open. 6s used to deterministically timeout slow-ICE peers into
    // an infinite retry loop.
    const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

    let mut timeout = Delay::new(HANDSHAKE_TIMEOUT).fuse();
    let mut future = Box::pin(future).fuse();

    select! {
        result = future => HandshakeOutcome::Result(result),
        _ = timeout => HandshakeOutcome::TimedOut(peer_id),
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
trait Messenger {
    type DataChannel: PeerDataSender;
    type HandshakeMeta: Send;

    async fn offer_handshake(
        signal_peer: SignalPeer,
        mut peer_signal_rx: UnboundedReceiver<PeerSignal>,
        messages_from_peers_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
        ice_server_config: &RtcIceServerConfig,
        channel_configs: &[ChannelConfig],
    ) -> HandshakeResult<Self::DataChannel, Self::HandshakeMeta>;

    async fn accept_handshake(
        signal_peer: SignalPeer,
        peer_signal_rx: UnboundedReceiver<PeerSignal>,
        messages_from_peers_tx: Vec<UnboundedSender<(PeerId, Packet)>>,
        ice_server_config: &RtcIceServerConfig,
        channel_configs: &[ChannelConfig],
    ) -> HandshakeResult<Self::DataChannel, Self::HandshakeMeta>;

    async fn peer_loop(peer_uuid: PeerId, handshake_meta: Self::HandshakeMeta) -> PeerId;
}

async fn message_loop<M: Messenger>(
    id_tx: futures_channel::oneshot::Sender<PeerId>,
    ice_server_config: &RtcIceServerConfig,
    channel_configs: &[ChannelConfig],
    channels: MessageLoopChannels,
    keep_alive_interval: Option<Duration>,
) -> Result<(), SignalingError> {
    const HANDSHAKE_RETRY_INTERVAL: Duration = Duration::from_millis(750);
    const HANDSHAKE_RETRY_MS: f64 = 750.0;

    let MessageLoopChannels {
        requests_sender,
        mut events_receiver,
        mut peer_messages_out_rx,
        messages_from_peers_tx,
        peer_state_tx,
        mut control_receiver,
        known_peer_tx,
        peer_left_tx,
    } = channels;

    let mut handshakes = FuturesUnordered::new();
    let mut peer_loops = FuturesUnordered::new();
    let mut handshake_signals = HashMap::new();
    let mut data_channels: HashMap<PeerId, Vec<M::DataChannel>> = HashMap::new();
    let mut known_peers = HashSet::new();
    let mut desired_peers: Option<HashSet<PeerId>> = None;
    let mut handshake_retry_after: HashMap<PeerId, f64> = HashMap::new();
    let mut id_tx = Option::Some(id_tx);
    let mut signaling_attached = true;

    let mut timeout = if let Some(interval) = keep_alive_interval {
        Either::Left(Delay::new(interval))
    } else {
        Either::Right(std::future::pending())
    }
    .fuse();
    let mut handshake_retry_tick = Delay::new(HANDSHAKE_RETRY_INTERVAL).fuse();

    macro_rules! queue_offer_handshake {
        ($peer:expr) => {{
            let peer_uuid = $peer;
            let should_connect = desired_peers
                .as_ref()
                .map_or(true, |set| set.contains(&peer_uuid));
            let retry_ready = handshake_retry_after
                .get(&peer_uuid)
                .map(|deadline| now_ms() >= *deadline)
                .unwrap_or(true);
            if signaling_attached
                && should_connect
                && retry_ready
                && !data_channels.contains_key(&peer_uuid)
                && !handshake_signals.contains_key(&peer_uuid)
            {
                let (signal_tx, signal_rx) = futures_channel::mpsc::unbounded();
                handshake_signals.insert(peer_uuid, signal_tx);
                handshake_retry_after.remove(&peer_uuid);
                let signal_peer = SignalPeer::new(peer_uuid, requests_sender.clone());
                handshakes.push(with_handshake_timeout(
                    peer_uuid,
                    M::offer_handshake(
                        signal_peer,
                        signal_rx,
                        messages_from_peers_tx.clone(),
                        ice_server_config,
                        channel_configs,
                    ),
                ));
            }
        }};
    }

    loop {
        let mut next_peer_messages_out = peer_messages_out_rx
            .iter_mut()
            .enumerate()
            .map(|(channel, rx)| async move { (channel, rx.next().await) })
            .collect::<FuturesUnordered<_>>();

        let mut next_peer_message_out = next_peer_messages_out.next().fuse();
        let mut next_signaling_event = if signaling_attached {
            Either::Left(events_receiver.next())
        } else {
            Either::Right(std::future::pending())
        }
        .fuse();

        select! {
            _  = &mut timeout => {
                if signaling_attached && requests_sender.unbounded_send(SignalingRequest::Peer(PeerRequest::KeepAlive)).is_err() {
                    // socket dropped
                    break Ok(());
                }
                if let Some(interval) = keep_alive_interval {
                    timeout = Either::Left(Delay::new(interval)).fuse();
                } else {
                    error!("no keep alive timeout, please file a bug");
                }
            }

            _ = &mut handshake_retry_tick => {
                if signaling_attached {
                    let retry_peers: Vec<PeerId> = known_peers.iter().copied().collect();
                    for peer_uuid in retry_peers {
                        queue_offer_handshake!(peer_uuid);
                    }
                }
                handshake_retry_tick = Delay::new(HANDSHAKE_RETRY_INTERVAL).fuse();
            }

            message = next_signaling_event => {
                if let Some(event) = message {
                    debug!("{event:?}");
                    match event {
                        PeerEvent::IdAssigned(peer_uuid) => {
                            if id_tx.take().expect("already sent peer id").send(peer_uuid.to_owned()).is_err() {
                                // Socket receiver was dropped, exit cleanly.
                                break Ok(());
                            };
                        },
                        PeerEvent::NewPeer(peer_uuid) => {
                            known_peers.insert(peer_uuid);
                            handshake_retry_after.remove(&peer_uuid);
                            if known_peer_tx.unbounded_send((peer_uuid, true)).is_err() {
                                break Ok(());
                            }
                            queue_offer_handshake!(peer_uuid);
                        },
                        PeerEvent::PeerLeft(peer_uuid) => {
                            known_peers.remove(&peer_uuid);
                            handshake_retry_after.remove(&peer_uuid);
                            let _ = known_peer_tx.unbounded_send((peer_uuid, false));
                            // Departure is broadcast room-wide by the server, so this is
                            // a reliable fast liveness signal for any member.
                            let _ = peer_left_tx.unbounded_send(peer_uuid);
                            handshake_signals.remove(&peer_uuid);
                            // Keep already-established data channels alive even if the peer leaves
                            // signaling; this allows a separate gameplay overlay from discovery.
                            if !data_channels.contains_key(&peer_uuid) {
                                if peer_state_tx.unbounded_send((peer_uuid, PeerState::Disconnected)).is_err() {
                                    // socket dropped, exit cleanly
                                    break Ok(());
                                }
                            }
                        },
                        PeerEvent::Signal { sender, data } => {
                            // Always accept incoming handshakes. Sparsity is
                            // enforced on the *initiating* side (desired-peer
                            // gating in queue_offer_handshake); rejecting
                            // accepts here only strands peers whose view of
                            // the topology is momentarily newer than ours.
                            // Surplus links are trimmed lazily by the session.
                            let signal_tx = handshake_signals.entry(sender).or_insert_with(|| {
                                let (from_peer_tx, peer_signal_rx) = futures_channel::mpsc::unbounded();
                                handshake_retry_after.remove(&sender);
                                let signal_peer = SignalPeer::new(sender, requests_sender.clone());
                                handshakes.push(with_handshake_timeout(
                                    sender,
                                    M::accept_handshake(
                                        signal_peer,
                                        peer_signal_rx,
                                        messages_from_peers_tx.clone(),
                                        ice_server_config,
                                        channel_configs,
                                    ),
                                ));
                                from_peer_tx
                            });

                            if signal_tx.unbounded_send(data).is_err() {
                                warn!("ignoring signal from peer {sender} because the handshake has already finished");
                            }
                        },
                    }
                } else if signaling_attached {
                    // Signaling stream closed (expected after detach or transient disconnect).
                    // Do not keep polling a closed receiver; that would spin and starve
                    // data-channel processing in the overlay loop.
                    signaling_attached = false;
                    warn!("signaling event stream closed; continuing with existing data channels");
                }
            }

            control = control_receiver.next().fuse() => {
                match control {
                    Some(SocketControl::FullMesh) => {
                        desired_peers = None;
                    }
                    Some(SocketControl::SetDesiredPeers(peers)) => {
                        debug!("set desired peers: {peers:?}");
                        desired_peers = Some(peers);
                    }
                    Some(SocketControl::DropPeer(peer)) => {
                        debug!("force drop peer: {peer:?}");
                        handshake_retry_after.remove(&peer);
                        if let Some(mut channels) = data_channels.remove(&peer) {
                            for channel in channels.iter_mut() {
                                channel.close();
                            }
                            let _ = peer_state_tx.unbounded_send((peer, PeerState::Disconnected));
                        }
                    }
                    Some(SocketControl::DetachSignaling) => {
                        if signaling_attached {
                            signaling_attached = false;
                            let _ = requests_sender.unbounded_send(SignalingRequest::Disconnect);
                            // Discovery is detached; only treat currently connected peers as known.
                            let connected: HashSet<PeerId> = data_channels.keys().copied().collect();
                            let stale: Vec<PeerId> = known_peers
                                .iter()
                                .filter(|peer| !connected.contains(peer))
                                .copied()
                                .collect();
                            for peer in stale {
                                known_peers.remove(&peer);
                                let _ = known_peer_tx.unbounded_send((peer, false));
                            }
                        }
                    }
                    None => {}
                }

                // Note: shrinking the desired set must NOT close established
                // channels. Live links are dropped only via an explicit
                // DropPeer (the session applies a make-before-break grace
                // window first); desired peers only gate new offers.

                if signaling_attached {
                    let known_snapshot: Vec<PeerId> = known_peers.iter().copied().collect();
                    for peer in known_snapshot {
                        queue_offer_handshake!(peer);
                    }
                }
            }

            handshake_result = handshakes.select_next_some() => {
                match handshake_result {
                    HandshakeOutcome::TimedOut(peer_id) => {
                        warn!("handshake timed out for peer {peer_id:?}");
                        handshake_signals.remove(&peer_id);
                        handshake_retry_after.insert(peer_id, now_ms() + HANDSHAKE_RETRY_MS);
                        let _ = peer_state_tx.unbounded_send((peer_id, PeerState::Disconnected));
                    }
                    HandshakeOutcome::Result(handshake_result) => {
                        let peer_id = handshake_result.peer_id;
                        handshake_signals.remove(&peer_id);
                        let mut channels = handshake_result.data_channels;
                        // Keep any successfully-established channel, even if the
                        // peer left the desired set mid-handshake: the session
                        // trims surplus links lazily (make-before-break).
                        if !handshake_result.established {
                            for channel in channels.iter_mut() {
                                channel.close();
                            }
                            handshake_retry_after.insert(peer_id, now_ms() + HANDSHAKE_RETRY_MS);
                            let _ = peer_state_tx.unbounded_send((peer_id, PeerState::Disconnected));
                            continue;
                        }
                        handshake_retry_after.remove(&peer_id);
                        data_channels.insert(peer_id, channels);
                        if peer_state_tx.unbounded_send((peer_id, PeerState::Connected)).is_err() {
                            // sending can only fail on socket drop, in which case connected_peers is unavailable, ignore
                            break Ok(());
                        }
                        peer_loops.push(M::peer_loop(peer_id, handshake_result.metadata));
                    }
                }
            }

            peer_uuid = peer_loops.select_next_some() => {
                debug!("peer {peer_uuid} finished");
                handshake_signals.remove(&peer_uuid);
                if let Some(mut channels) = data_channels.remove(&peer_uuid) {
                    for channel in channels.iter_mut() {
                        channel.close();
                    }
                }
                if signaling_attached
                    && desired_peers
                        .as_ref()
                        .map_or(true, |set| set.contains(&peer_uuid))
                    && known_peers.contains(&peer_uuid)
                {
                    handshake_retry_after
                        .insert(peer_uuid, now_ms() + HANDSHAKE_RETRY_MS);
                } else {
                    handshake_retry_after.remove(&peer_uuid);
                }
                if peer_state_tx.unbounded_send((peer_uuid, PeerState::Disconnected)).is_err() {
                    // sending can only fail on socket drop, in which case connected_peers is unavailable, ignore
                    break Ok(());
                }
            }

            message = next_peer_message_out => {
                match message {
                    Some((channel_index, Some((peer, packet)))) => {
                        let Some(channels) = data_channels.get_mut(&peer) else {
                            continue;
                        };
                        let Some(data_channel) = channels.get_mut(channel_index) else {
                            continue;
                        };
                        if let Err(e) = data_channel.send(packet) {
                            // Peer we're sending to closed their end of the connection.
                            // We anticipate the PeerLeft event soon, but we sent a message before it came.
                            // Do nothing. Only log it.
                            warn!("failed to send to peer {peer} (socket closed): {e:?}");
                        };
                    }
                    Some((_, None)) | None => {
                        // Receiver end of outgoing message channel closed,
                        // which most likely means the socket was dropped.
                        // There could probably be cleaner ways to handle this,
                        // but for now, just exit cleanly.
                        warn!("Outgoing message queue closed, message not sent");
                        break Ok(());
                    }
                }
            }

            complete => break Ok(())
        }
    }
}

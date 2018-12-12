use super::connection::Connection;
use bytes::{BufMut, Bytes, BytesMut};
use futures::task;
use futures::task::Task;
use futures::{Async, AsyncSink, Future, Poll, Sink, StartSend, Stream};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, VecDeque};
use std::io::{Error as IoError, ErrorKind, Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_io::{AsyncRead, AsyncWrite};

#[derive(PartialEq)]
pub enum StreamState {
    Open,
    Closing,
    Closed,
}

#[derive(Clone)]
pub struct DataMoneyStream {
    pub id: u64,
    pub money: MoneyStream,
    pub data: DataStream,
    state: Arc<RwLock<StreamState>>,
    connection: Arc<Connection>,
}

impl DataMoneyStream {
    pub fn close(&self) -> impl Future<Item = (), Error = ()> {
        CloseFuture {
            state: Arc::clone(&self.state),
            connection: Arc::clone(&self.connection),
        }
    }

    pub(super) fn new(id: u64, connection: Connection) -> DataMoneyStream {
        let state = Arc::new(RwLock::new(StreamState::Open));
        let connection = Arc::new(connection);
        DataMoneyStream {
            id,
            money: MoneyStream {
                connection: Arc::clone(&connection),
                state: Arc::clone(&state),
                send_max: Arc::new(AtomicUsize::new(0)),
                pending: Arc::new(AtomicUsize::new(0)),
                sent: Arc::new(AtomicUsize::new(0)),
                delivered: Arc::new(AtomicUsize::new(0)),
                received: Arc::new(AtomicUsize::new(0)),
                last_reported_received: Arc::new(AtomicUsize::new(0)),
                recv_task: Arc::new(Mutex::new(None)),
            },
            data: DataStream {
                connection: Arc::clone(&connection),
                state: Arc::clone(&state),
                incoming: Arc::new(Mutex::new(IncomingData {
                    offset: 0,
                    buffer: HashMap::new(),
                })),
                outgoing: Arc::new(Mutex::new(OutgoingData {
                    offset: 0,
                    buffer: VecDeque::new(),
                })),
                recv_task: Arc::new(Mutex::new(None)),
            },
            state: Arc::clone(&state),
            connection: Arc::clone(&connection),
        }
    }

    pub(super) fn is_closing(&self) -> bool {
        *self.state.read() == StreamState::Closing
    }

    pub(super) fn set_closing(&self) {
        *self.state.write() = StreamState::Closing;

        // Wake up both streams so they end
        self.money.try_wake_polling();
        self.data.try_wake_polling();
    }

    pub(super) fn set_closed(&self) {
        *self.state.write() = StreamState::Closed;

        // Wake up both streams so they end
        self.money.try_wake_polling();
        self.data.try_wake_polling();
    }
}

// TODO do we need a custom type just to implement this future?
pub struct CloseFuture {
    state: Arc<RwLock<StreamState>>,
    connection: Arc<Connection>,
}

impl Future for CloseFuture {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.connection.try_handle_incoming()?;

        if *self.state.read() == StreamState::Closed {
            Ok(Async::Ready(()))
        } else {
            *self.state.write() = StreamState::Closing;
            self.connection.try_send()?;
            Ok(Async::NotReady)
        }
    }
}

#[derive(Clone)]
pub struct MoneyStream {
    connection: Arc<Connection>,
    state: Arc<RwLock<StreamState>>,
    send_max: Arc<AtomicUsize>,
    pending: Arc<AtomicUsize>,
    sent: Arc<AtomicUsize>,
    delivered: Arc<AtomicUsize>,
    received: Arc<AtomicUsize>,
    last_reported_received: Arc<AtomicUsize>,
    recv_task: Arc<Mutex<Option<Task>>>,
}

impl MoneyStream {
    pub fn total_sent(&self) -> u64 {
        self.sent.load(Ordering::SeqCst) as u64
    }

    pub fn total_delivered(&self) -> u64 {
        self.delivered.load(Ordering::SeqCst) as u64
    }

    pub fn total_received(&self) -> u64 {
        self.received.load(Ordering::SeqCst) as u64
    }

    pub(super) fn pending(&self) -> u64 {
        self.pending.load(Ordering::SeqCst) as u64
    }

    pub(super) fn add_to_pending(&self, amount: u64) {
        self.pending.fetch_add(amount as usize, Ordering::SeqCst);
    }

    pub(super) fn subtract_from_pending(&self, amount: u64) {
        self.pending.fetch_sub(amount as usize, Ordering::SeqCst);
    }

    pub(super) fn pending_to_sent(&self, amount: u64) {
        self.pending.fetch_sub(amount as usize, Ordering::SeqCst);
        self.sent.fetch_add(amount as usize, Ordering::SeqCst);
    }

    pub(super) fn send_max(&self) -> u64 {
        self.send_max.load(Ordering::SeqCst) as u64
    }

    pub(super) fn add_received(&self, amount: u64) {
        self.received.fetch_add(amount as usize, Ordering::SeqCst);
    }

    pub(super) fn add_delivered(&self, amount: u64) {
        self.delivered.fetch_add(amount as usize, Ordering::SeqCst);
    }

    pub(super) fn try_wake_polling(&self) {
        if let Some(task) = (*self.recv_task.lock()).take() {
            debug!("Notifying MoneyStream poller that it should wake up");
            task.notify();
        }
    }
}

impl Stream for MoneyStream {
    type Item = u64;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.connection.try_handle_incoming()?;

        // Store the current task so that it can be woken up if the
        // DataStream happens to poll for incoming packets and gets data for us
        *self.recv_task.lock() = Some(task::current());

        let total_received = self.received.load(Ordering::SeqCst);
        let last_reported_received = self.last_reported_received.load(Ordering::SeqCst);
        let amount_received = total_received - last_reported_received;
        if amount_received > 0 {
            self.last_reported_received
                .store(total_received, Ordering::SeqCst);
            Ok(Async::Ready(Some(amount_received as u64)))
        } else if *self.state.read() != StreamState::Open {
            debug!("Money stream ended");
            Ok(Async::Ready(None))
        } else {
            Ok(Async::NotReady)
        }
    }
}

impl Sink for MoneyStream {
    type SinkItem = u64;
    type SinkError = ();

    fn start_send(&mut self, amount: u64) -> StartSend<Self::SinkItem, Self::SinkError> {
        if *self.state.read() != StreamState::Open {
            debug!("Cannot send money through stream because it is already closed or closing");
            return Err(());
        }
        self.send_max.fetch_add(amount as usize, Ordering::SeqCst);
        self.connection.try_send()?;
        Ok(AsyncSink::Ready)
    }

    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        self.connection.try_send()?;
        self.connection.try_handle_incoming()?;

        if self.sent.load(Ordering::SeqCst) >= self.send_max.load(Ordering::SeqCst) {
            Ok(Async::Ready(()))
        } else {
            Ok(Async::NotReady)
        }
    }
}

#[derive(Clone)]
pub struct DataStream {
    connection: Arc<Connection>,
    state: Arc<RwLock<StreamState>>,
    incoming: Arc<Mutex<IncomingData>>,
    outgoing: Arc<Mutex<OutgoingData>>,
    recv_task: Arc<Mutex<Option<Task>>>,
}

struct IncomingData {
    offset: usize,
    // TODO should we allow duplicate bytes and let the other side to resize chunks of data?
    // (we would need a sorted list instead of a HashMap to allow this)
    buffer: HashMap<usize, Bytes>,
}

struct OutgoingData {
    offset: usize,
    buffer: VecDeque<Bytes>,
}

impl DataStream {
    pub(super) fn push_incoming_data(&self, data: Bytes, offset: usize) -> Result<(), ()> {
        // TODO error if the buffer is too full
        // TODO don't block
        self.incoming.lock().buffer.insert(offset, data);
        Ok(())
    }

    pub(super) fn get_outgoing_data(&self, max_size: usize) -> Option<(Bytes, usize)> {
        let mut outgoing = self.outgoing.lock();
        // TODO make sure we're not copying data here
        let outgoing_offset = outgoing.offset;
        let mut chunks: Vec<Bytes> = Vec::new();
        let mut size: usize = 0;
        while size < max_size && !outgoing.buffer.is_empty() {
            let mut chunk = outgoing.buffer.pop_front()?;
            if chunk.len() >= max_size - size {
                chunks.push(chunk.split_to(max_size - size));
                size = max_size;

                if !chunk.is_empty() {
                    outgoing.buffer.push_front(chunk);
                }
            } else {
                size += chunk.len();
                chunks.push(chunk);
            }
        }

        if !chunks.is_empty() {
            // TODO zero copy
            let mut data = BytesMut::with_capacity(size);
            for chunk in chunks.iter() {
                data.put(chunk);
            }

            outgoing.offset += size;
            Some((data.freeze(), outgoing_offset))
        } else {
            None
        }
    }

    pub(super) fn try_wake_polling(&self) {
        if let Some(task) = (*self.recv_task.lock()).take() {
            debug!("Notifying the DataStream poller that it should wake up");
            task.notify();
        }
    }
}

impl Read for DataStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, IoError> {
        if buf.is_empty() {
            warn!("Asked to read into zero-length buffer");
        }

        self.connection.try_handle_incoming().map_err(|_| {
            IoError::new(
                ErrorKind::Other,
                "Error trying to handle incoming packets on Connection",
            )
        })?;

        // Store the current task so that it can be woken up if the
        // MoneyStream happens to poll for incoming packets and gets data for us
        *self.recv_task.lock() = Some(task::current());

        let mut incoming = self.incoming.lock();
        let incoming_offset = incoming.offset;
        if let Some(mut from_buf) = incoming.buffer.remove(&incoming_offset) {
            trace!("DataStream has incoming data");
            if from_buf.len() >= buf.len() {
                let to_copy = from_buf.split_to(buf.len());
                buf.copy_from_slice(&to_copy[..]);
                incoming.offset += to_copy.len();

                // Put the rest back in the queue
                if !from_buf.is_empty() {
                    let incoming_offset = incoming.offset;
                    incoming.buffer.insert(incoming_offset, from_buf);
                }

                incoming.offset += to_copy.len();
                trace!("Reading {} bytes of data", to_copy.len());
                Ok(to_copy.len())
            } else {
                let (mut buf_slice, _rest) = buf.split_at_mut(from_buf.len());
                buf_slice.copy_from_slice(&from_buf[..]);
                incoming.offset += from_buf.len();
                trace!("Reading {} bytes of data", from_buf.len());
                Ok(from_buf.len())
            }
        } else if *self.state.read() != StreamState::Open {
            debug!("Data stream ended");
            Ok(0)
        } else {
            Err(IoError::new(
                ErrorKind::WouldBlock,
                "No more data now but there might be more in the future",
            ))
        }
    }
}
impl AsyncRead for DataStream {}

impl Write for DataStream {
    fn write(&mut self, buf: &[u8]) -> Result<usize, IoError> {
        if *self.state.read() != StreamState::Open {
            debug!("Cannot write to stream because it is already closed or closing");
            return Err(IoError::new(
                ErrorKind::ConnectionReset,
                "Stream is already closed",
            ));
        }

        // TODO limit buffer size
        self.outgoing.lock().buffer.push_back(Bytes::from(buf));

        self.connection.try_send().map_err(|_| {
            IoError::new(ErrorKind::Other, "Error trying to send through Connection")
        })?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        // Try handling incoming packets in case the other side increased their limits
        self.connection.try_handle_incoming().map_err(|_| {
            IoError::new(
                ErrorKind::Other,
                "Error trying to handle incoming packets on Connection",
            )
        })?;

        self.connection.try_send().map_err(|_| {
            IoError::new(ErrorKind::Other, "Error trying to send through Connection")
        })?;

        let outgoing = self.outgoing.lock();
        if outgoing.buffer.is_empty() {
            Ok(())
        } else {
            Err(IoError::new(
                ErrorKind::WouldBlock,
                "Not finished sending yet",
            ))
        }
    }
}
impl AsyncWrite for DataStream {
    fn shutdown(&mut self) -> Result<Async<()>, IoError> {
        let mut state = self.state.write();
        if *state == StreamState::Closed {
            Ok(Async::Ready(()))
        } else {
            *state = StreamState::Closing;
            Err(IoError::new(ErrorKind::WouldBlock, "Stream is closing"))
        }
    }
}

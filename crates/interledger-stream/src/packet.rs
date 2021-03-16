use super::crypto::{decrypt, encrypt};
use byteorder::ReadBytesExt;
use bytes::{BufMut, BytesMut};
use interledger_packet::{
    oer::{BufOerExt, MutBufOerExt},
    Address, PacketType as IlpPacketType, ParseError,
};
#[cfg(test)]
use once_cell::sync::Lazy;
use std::{convert::TryFrom, fmt, str, u64};
use tracing::warn;

/// The Stream Protocol's version
const STREAM_VERSION: u8 = 1;

/// Builder for [Stream Packets](https://interledger.org/rfcs/0029-stream/#52-stream-packet)
pub struct StreamPacketBuilder<'a> {
    /// The stream packet's sequence number
    pub sequence: u64,
    /// The [ILP Packet Type](../interledger_packet/enum.PacketType.html)
    pub ilp_packet_type: IlpPacketType,
    /// Destination amount of the ILP Prepare, used for enforcing minimum exchange rates and congestion control.
    /// Within an ILP Prepare, represents the minimum amount the recipient needs to receive in order to fulfill the packet.
    /// Within an ILP Fulfill or ILP Reject, represents the amount received by the recipient.
    pub prepare_amount: u64,
    /// The stream frames
    pub frames: &'a [Frame<'a>],
}

impl<'a> StreamPacketBuilder<'a> {
    /// Serializes the builder into a Stream Packet
    pub fn build(&self) -> StreamPacket {
        let mut buffer_unencrypted = Vec::with_capacity(26);

        buffer_unencrypted.put_u8(STREAM_VERSION);
        buffer_unencrypted.put_u8(self.ilp_packet_type as u8);
        buffer_unencrypted.put_var_uint(self.sequence);
        buffer_unencrypted.put_var_uint(self.prepare_amount);
        buffer_unencrypted.put_var_uint(self.frames.len() as u64);
        let frames_offset = buffer_unencrypted.len();

        for frame in self.frames {
            let mut contents = Vec::new();
            match frame {
                Frame::ConnectionClose(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::ConnectionClose as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::ConnectionNewAddress(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::ConnectionNewAddress as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::ConnectionAssetDetails(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::ConnectionAssetDetails as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::ConnectionMaxData(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::ConnectionMaxData as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::ConnectionDataBlocked(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::ConnectionDataBlocked as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::ConnectionMaxStreamId(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::ConnectionMaxStreamId as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::ConnectionStreamIdBlocked(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::ConnectionStreamIdBlocked as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::StreamClose(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::StreamClose as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::StreamMoney(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::StreamMoney as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::StreamMaxMoney(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::StreamMaxMoney as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::StreamMoneyBlocked(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::StreamMoneyBlocked as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::StreamData(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::StreamData as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::StreamMaxData(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::StreamMaxData as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::StreamDataBlocked(ref frame) => {
                    buffer_unencrypted.put_u8(FrameType::StreamDataBlocked as u8);
                    frame.put_contents(&mut contents);
                }
                Frame::Unknown => continue,
            }
            buffer_unencrypted.put_var_octet_string(&*contents);
        }

        StreamPacket {
            buffer_unencrypted: BytesMut::from(&buffer_unencrypted[..]),
            sequence: self.sequence,
            ilp_packet_type: self.ilp_packet_type,
            prepare_amount: self.prepare_amount,
            frames_offset,
        }
    }
}

/// A Stream Packet as specified in its [ASN.1 definition](https://interledger.org/rfcs/asn1/Stream.asn)
#[derive(PartialEq, Clone)]
pub struct StreamPacket {
    /// The cleartext serialized packet
    pub(crate) buffer_unencrypted: BytesMut,
    /// The packet's sequence number
    sequence: u64,
    /// The [ILP Packet Type](../interledger_packet/enum.PacketType.html)
    ilp_packet_type: IlpPacketType,
    /// Destination amount of the ILP Prepare, used for enforcing minimum exchange rates and congestion control.
    /// Within an ILP Prepare, represents the minimum amount the recipient needs to receive in order to fulfill the packet.
    /// Within an ILP Fulfill or ILP Reject, represents the amount received by the recipient.
    prepare_amount: u64,
    /// The offset after which frames can be found inside the `buffer_unencrypted` field
    frames_offset: usize,
}

impl StreamPacket {
    /// Constructs a [Stream Packet](./struct.StreamPacket.html) from an encrypted buffer
    /// and a shared secret
    ///
    /// # Errors
    /// 1. If the version of Stream Protocol doesn't match the hardcoded [stream version](constant.STREAM_VERSION.html)
    /// 1. If the decryption fails
    /// 1. If the decrypted bytes cannot be parsed to an unencrypted [Stream Packet](./struct.StreamPacket.html)
    pub fn from_encrypted(shared_secret: &[u8], ciphertext: BytesMut) -> Result<Self, ParseError> {
        // TODO handle decryption failure
        let decrypted = decrypt(shared_secret, ciphertext)
            .map_err(|_err| ParseError::InvalidPacket(String::from("Unable to decrypt packet")))?;
        StreamPacket::from_bytes_unencrypted(decrypted)
    }

    #[cfg(fuzzing)]
    pub fn from_decrypted(data: BytesMut) -> Result<Self, ParseError> {
        Self::from_bytes_unencrypted(data)
    }

    /// Constructs a [Stream Packet](./struct.StreamPacket.html) from a buffer
    ///
    /// # Errors
    /// 1. If the version of Stream Protocol doesn't match the hardcoded [stream version](constant.STREAM_VERSION.html)
    /// 1. If the decrypted bytes cannot be parsed to an unencrypted [Stream Packet](./struct.StreamPacket.html)
    fn from_bytes_unencrypted(buffer_unencrypted: BytesMut) -> Result<Self, ParseError> {
        // TODO don't copy the whole packet again
        let mut reader = &buffer_unencrypted[..];
        let version = reader.read_u8()?;
        if version != STREAM_VERSION {
            return Err(ParseError::InvalidPacket(format!(
                "Unsupported STREAM version: {}",
                version
            )));
        }
        let ilp_packet_type = IlpPacketType::try_from(reader.read_u8()?)?;
        let sequence = reader.read_var_uint()?;
        let prepare_amount = reader.read_var_uint()?;

        // TODO save num_frames?
        let num_frames = reader.read_var_uint()?;
        let frames_offset = buffer_unencrypted.len() - reader.len();

        // Try reading through all the frames to make sure they can be parsed correctly
        if num_frames
            == (FrameIterator {
                buffer: &buffer_unencrypted[frames_offset..],
            })
            .count() as u64
        {
            Ok(StreamPacket {
                buffer_unencrypted,
                sequence,
                ilp_packet_type,
                prepare_amount,
                frames_offset,
            })
        } else {
            Err(ParseError::InvalidPacket(
                "Incorrect number of frames or unable to parse all frames".to_string(),
            ))
        }
    }

    /// Consumes the packet and a shared secret and returns a serialized encrypted
    /// Stream packet
    pub fn into_encrypted(self, shared_secret: &[u8]) -> BytesMut {
        encrypt(shared_secret, self.buffer_unencrypted)
    }

    /// The packet's sequence number
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// The packet's [type](../interledger_packet/enum.PacketType.html)
    pub fn ilp_packet_type(&self) -> IlpPacketType {
        self.ilp_packet_type
    }

    /// Destination amount of the ILP Prepare, used for enforcing minimum exchange rates and congestion control.
    pub fn prepare_amount(&self) -> u64 {
        self.prepare_amount
    }

    /// Returns a [FrameIterator](./struct.FrameIterator.html) over the packet's [frames](./enum.Frame.html)
    pub fn frames(&self) -> FrameIterator {
        FrameIterator {
            buffer: &self.buffer_unencrypted[self.frames_offset..],
        }
    }
}

impl fmt::Debug for StreamPacket {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "StreamPacket {{ sequence: {}, ilp_packet_type: {:?}, prepare_amount: {}, frames: {:?} }}",
            self.sequence,
            self.ilp_packet_type,
            self.prepare_amount,
            self.frames()
        )
    }
}

/// Iterator over a serialized Frame to support zero-copy deserialization
pub struct FrameIterator<'a> {
    buffer: &'a [u8],
}

impl<'a> FrameIterator<'a> {
    /// Reads a u8 from the iterator's buffer, and depending on the type it returns
    /// a [`Frame`](./enum.Frame.html)
    fn try_read_next_frame(&mut self) -> Result<Frame<'a>, ParseError> {
        let frame_type = self.buffer.read_u8()?;
        let contents: &'a [u8] = self.buffer.read_var_octet_string()?;
        let frame: Frame<'a> = match FrameType::from(frame_type) {
            FrameType::ConnectionClose => {
                Frame::ConnectionClose(ConnectionCloseFrame::read_contents(&contents)?)
            }
            FrameType::ConnectionNewAddress => {
                Frame::ConnectionNewAddress(ConnectionNewAddressFrame::read_contents(&contents)?)
            }
            FrameType::ConnectionAssetDetails => Frame::ConnectionAssetDetails(
                ConnectionAssetDetailsFrame::read_contents(&contents)?,
            ),
            FrameType::ConnectionMaxData => {
                Frame::ConnectionMaxData(ConnectionMaxDataFrame::read_contents(&contents)?)
            }
            FrameType::ConnectionDataBlocked => {
                Frame::ConnectionDataBlocked(ConnectionDataBlockedFrame::read_contents(&contents)?)
            }
            FrameType::ConnectionMaxStreamId => {
                Frame::ConnectionMaxStreamId(ConnectionMaxStreamIdFrame::read_contents(&contents)?)
            }
            FrameType::ConnectionStreamIdBlocked => Frame::ConnectionStreamIdBlocked(
                ConnectionStreamIdBlockedFrame::read_contents(&contents)?,
            ),
            FrameType::StreamClose => {
                Frame::StreamClose(StreamCloseFrame::read_contents(&contents)?)
            }
            FrameType::StreamMoney => {
                Frame::StreamMoney(StreamMoneyFrame::read_contents(&contents)?)
            }
            FrameType::StreamMaxMoney => {
                Frame::StreamMaxMoney(StreamMaxMoneyFrame::read_contents(&contents)?)
            }
            FrameType::StreamMoneyBlocked => {
                Frame::StreamMoneyBlocked(StreamMoneyBlockedFrame::read_contents(&contents)?)
            }
            FrameType::StreamData => Frame::StreamData(StreamDataFrame::read_contents(&contents)?),
            FrameType::StreamMaxData => {
                Frame::StreamMaxData(StreamMaxDataFrame::read_contents(&contents)?)
            }
            FrameType::StreamDataBlocked => {
                Frame::StreamDataBlocked(StreamDataBlockedFrame::read_contents(&contents)?)
            }
            FrameType::Unknown => {
                warn!(
                    "Ignoring unknown frame of type {}: {:x?}",
                    frame_type, contents,
                );
                Frame::Unknown
            }
        };

        Ok(frame)
    }
}

impl<'a> Iterator for FrameIterator<'a> {
    type Item = Frame<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.buffer.is_empty() {
            // TODO don't ignore errors if the packet is just invalid
            match self.try_read_next_frame() {
                Ok(frame) => return Some(frame),
                Err(err) => warn!("Error reading STREAM frame: {:?}", err),
            }
        }

        None
    }
}

impl<'a> fmt::Debug for FrameIterator<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "[ ")?;
        let mut iter = FrameIterator {
            buffer: self.buffer,
        };
        if let Some(next) = iter.next() {
            write!(f, "{:?}", next)?;
        }
        for frame in iter {
            write!(f, ", {:?}", frame)?;
        }
        write!(f, " ]")
    }
}

/// Enum around the different Stream Frame types
#[derive(PartialEq, Clone)]
pub enum Frame<'a> {
    ConnectionClose(ConnectionCloseFrame<'a>),
    ConnectionNewAddress(ConnectionNewAddressFrame),
    ConnectionAssetDetails(ConnectionAssetDetailsFrame<'a>),
    ConnectionMaxData(ConnectionMaxDataFrame),
    ConnectionDataBlocked(ConnectionDataBlockedFrame),
    ConnectionMaxStreamId(ConnectionMaxStreamIdFrame),
    ConnectionStreamIdBlocked(ConnectionStreamIdBlockedFrame),
    StreamClose(StreamCloseFrame<'a>),
    StreamMoney(StreamMoneyFrame),
    StreamMaxMoney(StreamMaxMoneyFrame),
    StreamMoneyBlocked(StreamMoneyBlockedFrame),
    StreamData(StreamDataFrame<'a>),
    StreamMaxData(StreamMaxDataFrame),
    StreamDataBlocked(StreamDataBlockedFrame),
    Unknown,
}

impl<'a> fmt::Debug for Frame<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Frame::ConnectionClose(frame) => write!(f, "{:?}", frame),
            Frame::ConnectionNewAddress(frame) => write!(f, "{:?}", frame),
            Frame::ConnectionAssetDetails(frame) => write!(f, "{:?}", frame),
            Frame::ConnectionMaxData(frame) => write!(f, "{:?}", frame),
            Frame::ConnectionDataBlocked(frame) => write!(f, "{:?}", frame),
            Frame::ConnectionMaxStreamId(frame) => write!(f, "{:?}", frame),
            Frame::ConnectionStreamIdBlocked(frame) => write!(f, "{:?}", frame),
            Frame::StreamClose(frame) => write!(f, "{:?}", frame),
            Frame::StreamMoney(frame) => write!(f, "{:?}", frame),
            Frame::StreamMaxMoney(frame) => write!(f, "{:?}", frame),
            Frame::StreamMoneyBlocked(frame) => write!(f, "{:?}", frame),
            Frame::StreamData(frame) => write!(f, "{:?}", frame),
            Frame::StreamMaxData(frame) => write!(f, "{:?}", frame),
            Frame::StreamDataBlocked(frame) => write!(f, "{:?}", frame),
            Frame::Unknown => write!(f, "UnknownFrame"),
        }
    }
}

/// The Stream Frame types [as defined in the RFC](https://interledger.org/rfcs/0029-stream/#53-frames)
#[derive(Debug, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum FrameType {
    ConnectionClose = 0x01,
    ConnectionNewAddress = 0x02,
    ConnectionMaxData = 0x03,
    ConnectionDataBlocked = 0x04,
    ConnectionMaxStreamId = 0x05,
    ConnectionStreamIdBlocked = 0x06,
    ConnectionAssetDetails = 0x07,
    StreamClose = 0x10,
    StreamMoney = 0x11,
    StreamMaxMoney = 0x12,
    StreamMoneyBlocked = 0x13,
    StreamData = 0x14,
    StreamMaxData = 0x15,
    StreamDataBlocked = 0x16,
    Unknown,
}

impl From<u8> for FrameType {
    fn from(num: u8) -> Self {
        match num {
            0x01 => FrameType::ConnectionClose,
            0x02 => FrameType::ConnectionNewAddress,
            0x03 => FrameType::ConnectionMaxData,
            0x04 => FrameType::ConnectionDataBlocked,
            0x05 => FrameType::ConnectionMaxStreamId,
            0x06 => FrameType::ConnectionStreamIdBlocked,
            0x07 => FrameType::ConnectionAssetDetails,
            0x10 => FrameType::StreamClose,
            0x11 => FrameType::StreamMoney,
            0x12 => FrameType::StreamMaxMoney,
            0x13 => FrameType::StreamMoneyBlocked,
            0x14 => FrameType::StreamData,
            0x15 => FrameType::StreamMaxData,
            0x16 => FrameType::StreamDataBlocked,
            _ => FrameType::Unknown,
        }
    }
}

/// The STREAM Error Codes [as defined in the RFC](https://interledger.org/rfcs/0029-stream/#54-error-codes)
#[derive(Debug, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum ErrorCode {
    NoError = 0x01,
    InternalError = 0x02,
    EndpointBusy = 0x03,
    FlowControlError = 0x04,
    StreamIdError = 0x05,
    StreamStateError = 0x06,
    FrameFormatError = 0x07,
    ProtocolViolation = 0x08,
    ApplicationError = 0x09,
    Unknown,
}
impl From<u8> for ErrorCode {
    fn from(num: u8) -> Self {
        match num {
            0x01 => ErrorCode::NoError,
            0x02 => ErrorCode::InternalError,
            0x03 => ErrorCode::EndpointBusy,
            0x04 => ErrorCode::FlowControlError,
            0x05 => ErrorCode::StreamIdError,
            0x06 => ErrorCode::StreamStateError,
            0x07 => ErrorCode::FrameFormatError,
            0x08 => ErrorCode::ProtocolViolation,
            0x09 => ErrorCode::ApplicationError,
            _ => ErrorCode::Unknown,
        }
    }
}

/// Helper trait for having a common interface to read/write on Frames
pub trait SerializableFrame<'a>: Sized {
    fn put_contents(&self, buf: &mut impl MutBufOerExt);

    fn read_contents(reader: &'a [u8]) -> Result<Self, ParseError>;
}

/// Frame after which a connection must be closed.
/// If implementations allow half-open connections, an endpoint may continue sending packets after receiving a ConnectionClose frame.
#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionCloseFrame<'a> {
    /// Machine-readable [Error Code](./enum.ErrorCode.html) indicating why the connection was closed.
    pub code: ErrorCode,
    /// Human-readable string intended to give more information helpful for debugging purposes.
    pub message: &'a str,
}

impl<'a> SerializableFrame<'a> for ConnectionCloseFrame<'a> {
    fn read_contents(mut reader: &'a [u8]) -> Result<Self, ParseError> {
        let code = ErrorCode::from(reader.read_u8()?);
        let message_bytes = reader.read_var_octet_string()?;
        let message = str::from_utf8(message_bytes)?;

        Ok(ConnectionCloseFrame { code, message })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_u8(self.code as u8);
        buf.put_var_octet_string(self.message.as_bytes());
    }
}

/// Frame which contains the sender of the Stream payment
#[derive(PartialEq, Clone)]
pub struct ConnectionNewAddressFrame {
    /// New ILP address of the endpoint that sent the frame.
    pub source_account: Address,
}

impl<'a> SerializableFrame<'a> for ConnectionNewAddressFrame {
    fn read_contents(mut reader: &'a [u8]) -> Result<Self, ParseError> {
        let source_account = reader.read_var_octet_string()?;
        let source_account = Address::try_from(source_account)?;

        Ok(ConnectionNewAddressFrame { source_account })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        let data: &[u8] = self.source_account.as_ref();
        buf.put_var_octet_string(data);
    }
}

impl<'a> fmt::Debug for ConnectionNewAddressFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "ConnectionNewAddressFrame {{ source_account: {} }}",
            self.source_account,
        )
    }
}

/// The assets being transported in this Stream payment
/// Asset details exposed by this frame MUST NOT change during the lifetime of a Connection.
#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionAssetDetailsFrame<'a> {
    /// Asset code of endpoint that sent the frame.
    pub source_asset_code: &'a str,
    /// Asset scale of endpoint that sent the frame.
    pub source_asset_scale: u8,
}

impl<'a> SerializableFrame<'a> for ConnectionAssetDetailsFrame<'a> {
    fn read_contents(mut reader: &'a [u8]) -> Result<Self, ParseError> {
        let source_asset_code = str::from_utf8(reader.read_var_octet_string()?)?;
        let source_asset_scale = reader.read_u8()?;

        Ok(ConnectionAssetDetailsFrame {
            source_asset_scale,
            source_asset_code,
        })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_octet_string(self.source_asset_code.as_bytes());
        buf.put_u8(self.source_asset_scale);
    }
}

/// Endpoints MUST NOT exceed the total number of bytes the other endpoint is willing to accept.
#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionMaxDataFrame {
    /// The total number of bytes the endpoint is willing to receive on this connection.
    pub max_offset: u64,
}

impl<'a> SerializableFrame<'a> for ConnectionMaxDataFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let max_offset = reader.read_var_uint()?;

        Ok(ConnectionMaxDataFrame { max_offset })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.max_offset);
    }
}

/// Frame specifying the amount of data which is going to be sent
#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionDataBlockedFrame {
    /// The total number of bytes the endpoint wants to send.
    pub max_offset: u64,
}

impl<'a> SerializableFrame<'a> for ConnectionDataBlockedFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let max_offset = reader.read_var_uint()?;

        Ok(ConnectionDataBlockedFrame { max_offset })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.max_offset);
    }
}

/// Frame specifying the maximum stream ID the endpoint is willing to accept.
#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionMaxStreamIdFrame {
    /// The maximum stream ID the endpoint is willing to accept.
    pub max_stream_id: u64,
}

impl<'a> SerializableFrame<'a> for ConnectionMaxStreamIdFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let max_stream_id = reader.read_var_uint()?;

        Ok(ConnectionMaxStreamIdFrame { max_stream_id })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.max_stream_id);
    }
}

/// Frame specifying the maximum stream ID the endpoint wishes to open.
#[derive(Debug, PartialEq, Clone)]
pub struct ConnectionStreamIdBlockedFrame {
    /// The maximum stream ID the endpoint wishes to open.
    pub max_stream_id: u64,
}

impl<'a> SerializableFrame<'a> for ConnectionStreamIdBlockedFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let max_stream_id = reader.read_var_uint()?;

        Ok(ConnectionStreamIdBlockedFrame { max_stream_id })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.max_stream_id);
    }
}

/// Endpoints MUST close the stream after receiving this stream immediately.
/// If implementations allow half-open streams, an endpoint MAY continue sending
/// money or data for this stream after receiving a StreamClose frame.
#[derive(Debug, PartialEq, Clone)]
pub struct StreamCloseFrame<'a> {
    /// Identifier of the stream this frame refers to.
    pub stream_id: u64,
    /// Machine-readable [Error Code](./enum.ErrorCode.html) indicating why the connection was closed.
    pub code: ErrorCode,
    /// Human-readable string intended to give more information helpful for debugging purposes.
    pub message: &'a str,
}

impl<'a> SerializableFrame<'a> for StreamCloseFrame<'a> {
    fn read_contents(mut reader: &'a [u8]) -> Result<Self, ParseError> {
        let stream_id = reader.read_var_uint()?;
        let code = ErrorCode::from(reader.read_u8()?);
        let message_bytes = reader.read_var_octet_string()?;
        let message = str::from_utf8(message_bytes)?;

        Ok(StreamCloseFrame {
            stream_id,
            code,
            message,
        })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.stream_id);
        buf.put_u8(self.code as u8);
        buf.put_var_octet_string(self.message.as_bytes());
    }
}

/// Frame specifying the amount of money that should go to each stream
///
/// The amount of money that should go to each stream is calculated by
/// dividing the number of shares for the given stream by the total number
/// of shares in all of the StreamMoney frames in the packet.
///
/// For example, if an ILP Prepare packet has an amount of 100 and three
/// StreamMoney frames with 5, 15, and 30 shares for streams 2, 4, and 6,
/// respectively, that would indicate that stream 2 should get 10 units,
/// stream 4 gets 30 units, and stream 6 gets 60 units.
/// If the Prepare amount is not divisible by the total number of shares,
/// stream amounts are rounded down.
///
/// The remainder is be allocated to the lowest-numbered open stream that has not reached its maximum receive amount.
#[derive(Debug, PartialEq, Clone)]
pub struct StreamMoneyFrame {
    /// Identifier of the stream this frame refers to.
    pub stream_id: u64,
    /// Proportion of the ILP Prepare amount destined for the stream specified.
    pub shares: u64,
}

impl<'a> SerializableFrame<'a> for StreamMoneyFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let stream_id = reader.read_var_uint()?;
        let shares = reader.read_var_uint()?;

        Ok(StreamMoneyFrame { stream_id, shares })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.stream_id);
        buf.put_var_uint(self.shares);
    }
}

/// Specifies the max amount of money the endpoint wants to send
///
/// The amounts in this frame are denominated in the units of the
/// endpoint sending the frame, so the other endpoint must use their
/// calculated exchange rate to determine how much more they can send
/// for this stream.
#[derive(Debug, PartialEq, Clone)]
pub struct StreamMaxMoneyFrame {
    /// Identifier of the stream this frame refers to.
    pub stream_id: u64,
    /// Total amount, denominated in the units of the endpoint
    /// sending this frame, that the endpoint is willing to receive on this stream.
    pub receive_max: u64,
    /// Total amount, denominated in the units of the endpoint
    /// sending this frame, that the endpoint has received thus far.
    pub total_received: u64,
}

impl<'a> SerializableFrame<'a> for StreamMaxMoneyFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let stream_id = reader.read_var_uint()?;
        let receive_max = saturating_read_var_uint(&mut reader)?;
        let total_received = reader.read_var_uint()?;

        Ok(StreamMaxMoneyFrame {
            stream_id,
            receive_max,
            total_received,
        })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.stream_id);
        buf.put_var_uint(self.receive_max);
        buf.put_var_uint(self.total_received);
    }
}

/// Frame specifying the maximum amount of money the sending endpoint will send
#[derive(Debug, PartialEq, Clone)]
pub struct StreamMoneyBlockedFrame {
    /// Identifier of the stream this frame refers to.
    pub stream_id: u64,
    /// Total amount, denominated in the units of the endpoint
    /// sending this frame, that the endpoint wants to send.
    pub send_max: u64,
    /// Total amount, denominated in the units of the endpoint
    /// sending this frame, that the endpoint has sent already.
    pub total_sent: u64,
}

impl<'a> SerializableFrame<'a> for StreamMoneyBlockedFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let stream_id = reader.read_var_uint()?;
        let send_max = saturating_read_var_uint(&mut reader)?;
        let total_sent = reader.read_var_uint()?;

        Ok(StreamMoneyBlockedFrame {
            stream_id,
            send_max,
            total_sent,
        })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.stream_id);
        buf.put_var_uint(self.send_max);
        buf.put_var_uint(self.total_sent);
    }
}

/// Packets may be received out of order so the Offset is used to
/// indicate the correct position of the byte segment in the overall stream.
/// The first StreamData frame sent for a given stream MUST start with an Offset of zero.

/// Fragments of data provided by a stream's StreamData frames
/// MUST NOT ever overlap with one another. For example, the following combination
/// of frames is forbidden because bytes 15-19 were provided twice:
///
/// ```ignore
/// StreamData { StreamID: 1, Offset: 10, Data: "1234567890" }
/// StreamData { StreamID: 1, Offset: 15, Data: "67890" }
/// ```
///
/// In other words, if a sender resends data (e.g. because a packet was lost),
/// it MUST resend the exact frames — offset and data.
/// This rule exists to simplify data reassembly for the receiver
#[derive(Debug, PartialEq, Clone)]
pub struct StreamDataFrame<'a> {
    /// Identifier of the stream this frame refers to.
    pub stream_id: u64,
    /// Position of this data in the byte stream.
    pub offset: u64,
    /// Application data
    pub data: &'a [u8],
}

impl<'a> SerializableFrame<'a> for StreamDataFrame<'a> {
    fn read_contents(mut reader: &'a [u8]) -> Result<Self, ParseError> {
        let stream_id = reader.read_var_uint()?;
        let offset = reader.read_var_uint()?;
        let data = reader.read_var_octet_string()?;

        Ok(StreamDataFrame {
            stream_id,
            offset,
            data,
        })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.stream_id);
        buf.put_var_uint(self.offset);
        buf.put_var_octet_string(self.data);
    }
}

/// The maximum amount of data the endpoint is willing to receive on this stream
#[derive(Debug, PartialEq, Clone)]
pub struct StreamMaxDataFrame {
    /// Identifier of the stream this frame refers to.
    pub stream_id: u64,
    /// The total number of bytes the endpoint is willing to receive on this stream.
    pub max_offset: u64,
}

impl<'a> SerializableFrame<'a> for StreamMaxDataFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let stream_id = reader.read_var_uint()?;
        let max_offset = reader.read_var_uint()?;

        Ok(StreamMaxDataFrame {
            stream_id,
            max_offset,
        })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.stream_id);
        buf.put_var_uint(self.max_offset);
    }
}

/// The maximum amount of data the endpoint is willing to send on this stream
#[derive(Debug, PartialEq, Clone)]
pub struct StreamDataBlockedFrame {
    /// Identifier of the stream this frame refers to.
    pub stream_id: u64,
    /// The total number of bytes the endpoint wants to send on this stream.
    pub max_offset: u64,
}

impl<'a> SerializableFrame<'a> for StreamDataBlockedFrame {
    fn read_contents(mut reader: &[u8]) -> Result<Self, ParseError> {
        let stream_id = reader.read_var_uint()?;
        let max_offset = reader.read_var_uint()?;

        Ok(StreamDataBlockedFrame {
            stream_id,
            max_offset,
        })
    }

    fn put_contents(&self, buf: &mut impl MutBufOerExt) {
        buf.put_var_uint(self.stream_id);
        buf.put_var_uint(self.max_offset);
    }
}

/// See: https://github.com/interledger/rfcs/blob/master/0029-stream/0029-stream.md#514-maximum-varuint-size
fn saturating_read_var_uint<'a>(reader: &mut impl BufOerExt<'a>) -> Result<u64, ParseError> {
    if reader.peek_var_octet_string()?.len() > 8 {
        reader.skip_var_octet_string()?;
        Ok(u64::MAX)
    } else {
        Ok(reader.read_var_uint()?)
    }
}

#[cfg(test)]
mod serialization {
    use super::*;
    use std::str::FromStr;

    static PACKET: Lazy<StreamPacket> = Lazy::new(|| {
        StreamPacketBuilder {
            sequence: 1,
            ilp_packet_type: IlpPacketType::try_from(12).unwrap(),
            prepare_amount: 99,
            frames: &[
                Frame::ConnectionClose(ConnectionCloseFrame {
                    code: ErrorCode::NoError,
                    message: "oop",
                }),
                Frame::ConnectionNewAddress(ConnectionNewAddressFrame {
                    source_account: Address::from_str("example.blah").unwrap(),
                }),
                Frame::ConnectionMaxData(ConnectionMaxDataFrame { max_offset: 1000 }),
                Frame::ConnectionDataBlocked(ConnectionDataBlockedFrame { max_offset: 2000 }),
                Frame::ConnectionMaxStreamId(ConnectionMaxStreamIdFrame {
                    max_stream_id: 3000,
                }),
                Frame::ConnectionStreamIdBlocked(ConnectionStreamIdBlockedFrame {
                    max_stream_id: 4000,
                }),
                Frame::ConnectionAssetDetails(ConnectionAssetDetailsFrame {
                    source_asset_code: "XYZ",
                    source_asset_scale: 9,
                }),
                Frame::StreamClose(StreamCloseFrame {
                    stream_id: 76,
                    code: ErrorCode::InternalError,
                    message: "blah",
                }),
                Frame::StreamMoney(StreamMoneyFrame {
                    stream_id: 88,
                    shares: 99,
                }),
                Frame::StreamMaxMoney(StreamMaxMoneyFrame {
                    stream_id: 11,
                    receive_max: 987,
                    total_received: 500,
                }),
                Frame::StreamMoneyBlocked(StreamMoneyBlockedFrame {
                    stream_id: 66,
                    send_max: 20000,
                    total_sent: 6000,
                }),
                Frame::StreamData(StreamDataFrame {
                    stream_id: 34,
                    offset: 9000,
                    data: b"hello",
                }),
                Frame::StreamMaxData(StreamMaxDataFrame {
                    stream_id: 35,
                    max_offset: 8766,
                }),
                Frame::StreamDataBlocked(StreamDataBlockedFrame {
                    stream_id: 888,
                    max_offset: 44444,
                }),
            ],
        }
        .build()
    });
    static SERIALIZED: Lazy<BytesMut> = Lazy::new(|| {
        BytesMut::from(
            &vec![
                1, 12, 1, 1, 1, 99, 1, 14, 1, 5, 1, 3, 111, 111, 112, 2, 13, 12, 101, 120, 97, 109,
                112, 108, 101, 46, 98, 108, 97, 104, 3, 3, 2, 3, 232, 4, 3, 2, 7, 208, 5, 3, 2, 11,
                184, 6, 3, 2, 15, 160, 7, 5, 3, 88, 89, 90, 9, 16, 8, 1, 76, 2, 4, 98, 108, 97,
                104, 17, 4, 1, 88, 1, 99, 18, 8, 1, 11, 2, 3, 219, 2, 1, 244, 19, 8, 1, 66, 2, 78,
                32, 2, 23, 112, 20, 11, 1, 34, 2, 35, 40, 5, 104, 101, 108, 108, 111, 21, 5, 1, 35,
                2, 34, 62, 22, 6, 2, 3, 120, 2, 173, 156,
            ][..],
        )
    });

    #[test]
    fn it_serializes_to_same_as_javascript() {
        assert_eq!(PACKET.buffer_unencrypted, *SERIALIZED);
    }

    #[test]
    fn it_deserializes_from_javascript() {
        assert_eq!(
            StreamPacket::from_bytes_unencrypted(SERIALIZED.clone()).unwrap(),
            *PACKET
        );
    }

    #[test]
    fn it_iterates_through_the_frames() {
        let mut iter = PACKET.frames();
        assert_eq!(
            iter.next().unwrap(),
            Frame::ConnectionClose(ConnectionCloseFrame {
                code: ErrorCode::NoError,
                message: "oop"
            })
        );

        assert_eq!(
            iter.next().unwrap(),
            Frame::ConnectionNewAddress(ConnectionNewAddressFrame {
                source_account: Address::from_str("example.blah").unwrap()
            })
        );
        assert_eq!(iter.count(), 12);
    }

    #[test]
    fn it_saturates_max_money_frame_receive_max() {
        let mut buffer = BytesMut::new();
        buffer.put_var_uint(123); // stream_id
        buffer.put_var_octet_string(
            &[
                // receive_max
                0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
            ][..],
        );
        buffer.put_var_uint(123); // total_received
        let frame = StreamMaxMoneyFrame::read_contents(&buffer).unwrap();
        assert_eq!(frame.receive_max, u64::MAX);
    }

    #[test]
    fn it_saturates_money_blocked_frame_send_max() {
        let mut buffer = BytesMut::new();
        buffer.put_var_uint(123); // stream_id
        buffer.put_var_octet_string(
            &[
                // send_max
                0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
            ][..],
        );
        buffer.put_var_uint(123); // total_sent
        let frame = StreamMoneyBlockedFrame::read_contents(&buffer).unwrap();
        assert_eq!(frame.send_max, u64::MAX);
    }
}

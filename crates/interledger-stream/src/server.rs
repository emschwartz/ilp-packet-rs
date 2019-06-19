use super::crypto::*;
use super::packet::*;
use base64;
use bytes::Bytes;
use futures::future::result;
use hex;
use interledger_ildcp::IldcpAccount;
use interledger_packet::{
    Address, ErrorCode, Fulfill, FulfillBuilder, PacketType as IlpPacketType, Prepare, Reject,
    RejectBuilder,
};
use interledger_service::{Account, BoxedIlpFuture, OutgoingRequest, OutgoingService};
use std::convert::TryFrom;
use std::marker::PhantomData;

const STREAM_SERVER_SECRET_GENERATOR: &[u8] = b"ilp_stream_secret_generator";

/// A STREAM connection generator that creates `destination_account` and `shared_secret` values
/// based on a single root secret.
///
/// This can be reused across multiple STREAM connections so that a single receiver can
/// accept incoming packets for multiple connections.
#[derive(Clone)]
pub struct ConnectionGenerator {
    secret_generator: Bytes,
}

impl ConnectionGenerator {
    pub fn new(server_secret: Bytes) -> Self {
        assert_eq!(server_secret.len(), 32, "Server secret must be 32 bytes");
        ConnectionGenerator {
            secret_generator: Bytes::from(
                &hmac_sha256(&server_secret[..], STREAM_SERVER_SECRET_GENERATOR)[..],
            ),
        }
    }

    /// Generate the STREAM parameters for the given ILP address and the configured server secret.
    ///
    /// The `destination_account` is generated such that the `shared_secret` can be re-derived
    /// from a Prepare packet's destination and the same server secret. If the address is modified
    /// in any way, the server will not be able to re-derive the secret and the packet will be rejected.
    // TODO make sure this is an ILP address
    pub fn generate_address_and_secret(&self, base_address: &Address) -> (Address, [u8; 32]) {
        let random_bytes = generate_token();
        // base_address + "." + 32-bytes encoded as base64url
        let shared_secret = hmac_sha256(&self.secret_generator[..], &random_bytes[..]);
        // Note that the unwrap here is safe because we know the base_address
        // is valid and adding base64-url characters will always be valid
        let destination_account = base_address
            .with_suffix(
                &base64::encode_config(&random_bytes[..], base64::URL_SAFE_NO_PAD).as_ref(),
            )
            .unwrap();

        let auth_tag = &hmac_sha256(&shared_secret[..], destination_account.as_ref())[..14];

        // can we avoid the copy?
        let mut dest = destination_account.to_bytes();
        dest.extend(base64::encode_config(auth_tag, base64::URL_SAFE_NO_PAD).bytes());
        let destination_account = Address::try_from(dest).unwrap();

        debug!("Generated address: {}", destination_account,);
        (destination_account, shared_secret)
    }

    /// Rederive the `shared_secret` from a `destination_account`. This will return an
    /// error if the address has been modified in any way or if the packet was not generated
    /// with the same server secret.
    pub fn rederive_secret(&self, destination_account: &Address) -> Result<[u8; 32], ()> {
        let local_part = destination_account.segments().rev().next().unwrap();
        let local_part =
            base64::decode_config(local_part, base64::URL_SAFE_NO_PAD).map_err(|_| ())?;
        if local_part.len() == 32 {
            let (random_bytes, auth_tag) = local_part.split_at(18);
            let shared_secret = hmac_sha256(&self.secret_generator[..], &random_bytes[..]);
            let dest: &[u8] = destination_account.as_ref();
            let derived_auth_tag = &hmac_sha256(&shared_secret[..], &dest[..dest.len() - 19])[..14];
            if derived_auth_tag == auth_tag {
                return Ok(shared_secret);
            } else {
                warn!("Got packet where auth tag doesn't match. Expected: {}, actual: {}, destination_account: {:?}",
                base64::encode_config(derived_auth_tag, base64::URL_SAFE_NO_PAD),
                base64::encode_config(auth_tag, base64::URL_SAFE_NO_PAD),
                destination_account)
            }
        }
        Err(())
    }
}

/// An OutgoingService that fulfills incoming STREAM packets.
///
/// Note this does **not** maintain STREAM state, but instead fulfills
/// all incoming packets to collect the money.
///
/// This does not currently support handling data sent via STREAM.
#[derive(Clone)]
pub struct StreamReceiverService<O: OutgoingService<A>, A: Account> {
    connection_generator: ConnectionGenerator,
    next: O,
    account_type: PhantomData<A>,
}

impl<O, A> StreamReceiverService<O, A>
where
    O: OutgoingService<A>,
    A: Account,
{
    pub fn new(server_secret: Bytes, next: O) -> Self {
        let connection_generator = ConnectionGenerator::new(server_secret);
        StreamReceiverService {
            connection_generator,
            next,
            account_type: PhantomData,
        }
    }
}

// TODO should this be an OutgoingService instead so the balance logic is applied before this is called?
impl<O, A> OutgoingService<A> for StreamReceiverService<O, A>
where
    O: OutgoingService<A>,
    A: Account + IldcpAccount,
{
    type Future = BoxedIlpFuture;

    /// Try fulfilling the request if it is for this STREAM server or pass it to the next
    /// outgoing handler if not.
    ///
    /// The method used for generating the `destination_account` and `shared_secret` enables
    /// the server to check whether the Prepare packet was created with STREAM parameters
    /// that this server would have created or not.
    fn send_request(&mut self, request: OutgoingRequest<A>) -> Self::Future {
        let dest = request.prepare.destination();
        let dest: &[u8] = dest.as_ref();
        let to = request.to.client_address();
        if dest.as_ref().starts_with(to.as_ref()) {
            if let Ok(shared_secret) = self
                .connection_generator
                .rederive_secret(&request.prepare.destination())
            {
                {
                    return Box::new(result(receive_money(&shared_secret, &to, request.prepare)));
                }
            }
        }
        Box::new(self.next.send_request(request))
    }
}

// TODO send asset code and scale back to sender also
fn receive_money(
    shared_secret: &[u8; 32],
    client_address: &Address,
    prepare: Prepare,
) -> Result<Fulfill, Reject> {
    // Generate fulfillment
    let fulfillment = generate_fulfillment(&shared_secret[..], prepare.data());
    let condition = hash_sha256(&fulfillment);
    let is_fulfillable = condition == prepare.execution_condition();

    // Parse STREAM packet
    // TODO avoid copying data
    let prepare_amount = prepare.amount();
    let stream_packet =
        StreamPacket::from_encrypted(shared_secret, prepare.into_data()).map_err(|_| {
            debug!("Unable to parse data, rejecting Prepare packet");
            RejectBuilder {
                code: ErrorCode::F06_UNEXPECTED_PAYMENT,
                message: b"Could not decrypt data",
                triggered_by: Some(client_address),
                data: &[],
            }
            .build()
        })?;

    let mut response_frames: Vec<Frame> = Vec::new();

    // Handle STREAM frames
    // TODO reject if they send data?
    for frame in stream_packet.frames() {
        // Tell the sender the stream can handle lots of money
        if let Frame::StreamMoney(frame) = frame {
            response_frames.push(Frame::StreamMaxMoney(StreamMaxMoneyFrame {
                stream_id: frame.stream_id,
                // TODO will returning zero here cause problems?
                total_received: 0,
                receive_max: u64::max_value(),
            }));
        }
    }

    // Return Fulfill or Reject Packet
    if is_fulfillable && prepare_amount >= stream_packet.prepare_amount() {
        let response_packet = StreamPacketBuilder {
            sequence: stream_packet.sequence(),
            ilp_packet_type: IlpPacketType::Fulfill,
            prepare_amount,
            frames: &response_frames,
        }
        .build();
        debug!(
            "Fulfilling prepare with fulfillment: {} and encrypted stream packet: {:?}",
            hex::encode(&fulfillment[..]),
            response_packet
        );
        let encrypted_response = response_packet.into_encrypted(shared_secret);
        let fulfill = FulfillBuilder {
            fulfillment: &fulfillment,
            data: &encrypted_response[..],
        }
        .build();
        Ok(fulfill)
    } else {
        let response_packet = StreamPacketBuilder {
            sequence: stream_packet.sequence(),
            ilp_packet_type: IlpPacketType::Reject,
            prepare_amount,
            frames: &response_frames,
        }
        .build();
        if !is_fulfillable {
            debug!("Packet is unfulfillable");
        } else if prepare_amount < stream_packet.prepare_amount() {
            debug!(
                "Received only: {} when we should have received at least: {}",
                prepare_amount,
                stream_packet.prepare_amount()
            );
        }
        debug!(
            "Rejecting Prepare and including encrypted stream packet {:?}",
            response_packet
        );
        let encrypted_response = response_packet.into_encrypted(shared_secret);
        let reject = RejectBuilder {
            code: ErrorCode::F99_APPLICATION_ERROR,
            message: &[],
            triggered_by: Some(&client_address),
            data: &encrypted_response[..],
        }
        .build();
        Err(reject)
    }
}

#[cfg(test)]
mod connection_generator {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn generates_valid_ilp_address() {
        let server_secret = [9; 32];
        let receiver_address = Address::from_str("example.receiver").unwrap();
        let connection_generator = ConnectionGenerator::new(Bytes::from(&server_secret[..]));
        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&receiver_address);

        assert!(destination_account
            .to_bytes()
            .starts_with(receiver_address.as_ref()));

        assert_eq!(
            connection_generator
                .rederive_secret(&destination_account)
                .unwrap(),
            shared_secret
        );
    }

    #[test]
    fn errors_if_it_cannot_rederive_secret() {
        let server_secret = [9; 32];
        let receiver_address = Address::from_str("example.receiver").unwrap();
        let connection_generator = ConnectionGenerator::new(Bytes::from(&server_secret[..]));
        let (destination_account, _shared_secret) =
            connection_generator.generate_address_and_secret(&receiver_address);

        let destination_account = destination_account.with_suffix(b"extra").unwrap();

        assert!(connection_generator
            .rederive_secret(&destination_account)
            .is_err());
    }
}

#[cfg(test)]
fn test_stream_packet() -> StreamPacket {
    StreamPacketBuilder {
        ilp_packet_type: IlpPacketType::Prepare,
        prepare_amount: 0,
        sequence: 1,
        frames: &[Frame::StreamMoney(StreamMoneyFrame {
            stream_id: 1,
            shares: 1,
        })],
    }
    .build()
}

#[cfg(test)]
mod receiving_money {
    use super::*;
    use interledger_packet::PrepareBuilder;

    use std::str::FromStr;
    use std::time::UNIX_EPOCH;
    #[test]
    fn fulfills_valid_packet() {
        let client_address = Address::from_str("example.destination").unwrap();
        let server_secret = Bytes::from(&[1; 32][..]);
        let connection_generator = ConnectionGenerator::new(server_secret.clone());
        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&client_address);
        let stream_packet = test_stream_packet();
        let data = stream_packet.into_encrypted(&shared_secret[..]);
        let execution_condition = generate_condition(&shared_secret[..], &data);

        let dest = Address::try_from(destination_account).unwrap();
        let prepare = PrepareBuilder {
            destination: dest,
            amount: 100,
            expires_at: UNIX_EPOCH,
            data: &data[..],
            execution_condition: &execution_condition,
        }
        .build();

        let shared_secret = connection_generator
            .rederive_secret(&prepare.destination())
            .unwrap();
        let result = receive_money(&shared_secret, &client_address, prepare);
        assert!(result.is_ok());
    }

    #[test]
    fn fulfills_valid_packet_without_connection_tag() {
        let client_address = Address::from_str("example.destination").unwrap();
        let server_secret = Bytes::from(&[1; 32][..]);
        let connection_generator = ConnectionGenerator::new(server_secret.clone());
        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&client_address);
        let stream_packet = test_stream_packet();
        let data = stream_packet.into_encrypted(&shared_secret[..]);
        let execution_condition = generate_condition(&shared_secret[..], &data);

        let dest = Address::try_from(destination_account).unwrap();
        let prepare = PrepareBuilder {
            destination: dest,
            amount: 100,
            expires_at: UNIX_EPOCH,
            data: &data[..],
            execution_condition: &execution_condition,
        }
        .build();

        let shared_secret = connection_generator
            .rederive_secret(&prepare.destination())
            .unwrap();
        let result = receive_money(&shared_secret, &client_address, prepare);
        assert!(result.is_ok());
    }

    #[test]
    fn rejects_modified_data() {
        let client_address = Address::from_str("example.destination").unwrap();
        let server_secret = Bytes::from(&[1; 32][..]);
        let connection_generator = ConnectionGenerator::new(server_secret.clone());
        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&client_address);
        let stream_packet = test_stream_packet();
        let mut data = stream_packet.into_encrypted(&shared_secret[..]);
        data.extend_from_slice(b"x");
        let execution_condition = generate_condition(&shared_secret[..], &data);

        let dest = Address::try_from(destination_account).unwrap();
        let prepare = PrepareBuilder {
            destination: dest,
            amount: 100,
            expires_at: UNIX_EPOCH,
            data: &data[..],
            execution_condition: &execution_condition,
        }
        .build();

        let shared_secret = connection_generator
            .rederive_secret(&prepare.destination())
            .unwrap();
        let result = receive_money(&shared_secret, &client_address, prepare);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_too_little_money() {
        let client_address = Address::from_str("example.destination").unwrap();
        let server_secret = Bytes::from(&[1; 32][..]);
        let connection_generator = ConnectionGenerator::new(server_secret.clone());
        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&client_address);

        let stream_packet = StreamPacketBuilder {
            ilp_packet_type: IlpPacketType::Prepare,
            prepare_amount: 101,
            sequence: 1,
            frames: &[Frame::StreamMoney(StreamMoneyFrame {
                stream_id: 1,
                shares: 1,
            })],
        }
        .build();

        let data = stream_packet.into_encrypted(&shared_secret[..]);
        let execution_condition = generate_condition(&shared_secret[..], &data);

        let dest = Address::try_from(destination_account).unwrap();
        let prepare = PrepareBuilder {
            destination: dest,
            amount: 100,
            expires_at: UNIX_EPOCH,
            data: &data[..],
            execution_condition: &execution_condition,
        }
        .build();

        let shared_secret = connection_generator
            .rederive_secret(&prepare.destination())
            .unwrap();
        let result = receive_money(&shared_secret, &client_address, prepare);
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod stream_receiver_service {
    use super::*;
    use crate::test_helpers::*;
    use futures::Future;
    use interledger_packet::PrepareBuilder;
    use interledger_service::outgoing_service_fn;

    use std::str::FromStr;
    use std::time::UNIX_EPOCH;
    #[test]
    fn fulfills_correct_packets() {
        let client_address = Address::from_str("example.destination").unwrap();
        let server_secret = Bytes::from(&[1; 32][..]);
        let connection_generator = ConnectionGenerator::new(server_secret.clone());
        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&client_address);
        let stream_packet = test_stream_packet();
        let data = stream_packet.into_encrypted(&shared_secret[..]);
        let execution_condition = generate_condition(&shared_secret[..], &data);

        let dest = Address::try_from(destination_account).unwrap();
        let prepare = PrepareBuilder {
            destination: dest,
            amount: 100,
            expires_at: UNIX_EPOCH,
            data: &data[..],
            execution_condition: &execution_condition,
        }
        .build();

        let mut service = StreamReceiverService::new(
            server_secret.clone(),
            outgoing_service_fn(|_: OutgoingRequest<TestAccount>| -> BoxedIlpFuture {
                panic!("shouldn't get here")
            }),
        );

        let result = service
            .send_request(OutgoingRequest {
                from: TestAccount {
                    id: 0,
                    ilp_address: Address::from_str("example.sender").unwrap(),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9,
                },
                to: TestAccount {
                    id: 1,
                    ilp_address: client_address.clone(),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9,
                },
                original_amount: prepare.amount(),
                prepare,
            })
            .wait();
        assert!(result.is_ok());
    }

    #[test]
    fn rejects_invalid_packets() {
        let client_address = Address::from_str("example.destination").unwrap();
        let server_secret = Bytes::from(&[1; 32][..]);
        let connection_generator = ConnectionGenerator::new(server_secret.clone());
        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&client_address);
        let stream_packet = test_stream_packet();
        let mut data = stream_packet.into_encrypted(&shared_secret[..]);
        let execution_condition = generate_condition(&shared_secret[..], &data);

        data.extend_from_slice(b"extra");
        let dest = Address::try_from(destination_account).unwrap();

        let prepare = PrepareBuilder {
            destination: dest,
            amount: 100,
            expires_at: UNIX_EPOCH,
            data: &data[..],
            execution_condition: &execution_condition,
        }
        .build();

        let mut service = StreamReceiverService::new(
            server_secret.clone(),
            outgoing_service_fn(|_: OutgoingRequest<TestAccount>| -> BoxedIlpFuture {
                panic!("shouldn't get here")
            }),
        );

        let result = service
            .send_request(OutgoingRequest {
                from: TestAccount {
                    id: 0,
                    ilp_address: Address::from_str("example.sender").unwrap(),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9,
                },
                to: TestAccount {
                    id: 1,
                    ilp_address: client_address.clone(),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9,
                },
                original_amount: prepare.amount(),
                prepare,
            })
            .wait();
        assert!(result.is_err());
    }

    #[test]
    fn passes_on_packets_not_for_it() {
        let client_address = Address::from_str("example.destination").unwrap();
        let server_secret = Bytes::from(&[1; 32][..]);
        let connection_generator = ConnectionGenerator::new(server_secret.clone());
        let (destination_account, shared_secret) =
            connection_generator.generate_address_and_secret(&client_address);
        let stream_packet = test_stream_packet();
        let data = stream_packet.into_encrypted(&shared_secret[..]);
        let execution_condition = generate_condition(&shared_secret[..], &data);

        let dest = Address::try_from(destination_account).unwrap();
        let dest = dest.with_suffix(b"extra").unwrap();

        let prepare = PrepareBuilder {
            destination: dest,
            amount: 100,
            expires_at: UNIX_EPOCH,
            data: &data[..],
            execution_condition: &execution_condition,
        }
        .build();

        let mut service = StreamReceiverService::new(
            server_secret.clone(),
            outgoing_service_fn(|_| {
                Err(RejectBuilder {
                    code: ErrorCode::F02_UNREACHABLE,
                    message: &[],
                    data: &[],
                    triggered_by: Address::from_str("example.other-receiver").ok().as_ref(),
                }
                .build())
            }),
        );

        let result = service
            .send_request(OutgoingRequest {
                from: TestAccount {
                    id: 0,
                    ilp_address: Address::from_str("example.sender").unwrap(),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9,
                },
                original_amount: prepare.amount(),
                to: TestAccount {
                    id: 1,
                    ilp_address: client_address.clone(),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9,
                },
                prepare,
            })
            .wait();
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().triggered_by().unwrap(),
            Address::from_str("example.other-receiver").unwrap(),
        );
    }
}

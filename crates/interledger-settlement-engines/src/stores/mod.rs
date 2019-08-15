use bytes::Bytes;
use futures::future::Future;
use http::StatusCode;

pub mod redis_ethereum_ledger;
pub mod redis_store_common;

#[cfg(test)]
pub mod test_helpers;

pub type IdempotentEngineData = (StatusCode, Bytes, [u8; 32]);

pub trait IdempotentEngineStore {
    /// Returns the API response that was saved when the idempotency key was used
    /// Also returns a hash of the input data which resulted in the response
    fn load_idempotent_data(
        &self,
        idempotency_key: String,
    ) -> Box<dyn Future<Item = Option<IdempotentEngineData>, Error = ()> + Send>;

    /// Saves the data that was passed along with the api request for later
    /// The store MUST also save a hash of the input, so that it errors out on requests
    fn save_idempotent_data(
        &self,
        idempotency_key: String,
        input_hash: [u8; 32],
        status_code: StatusCode,
        data: Bytes,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;
}

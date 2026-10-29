//! What the keeper needs from the chain.

use varswap_sdk::Address;

use crate::{KeeperError, PrintMsg};

/// The keeper's view of the chain for one deployment: an RPC client in production, LiteSVM in
/// tests.
pub trait Chain {
    /// Current cluster time (Clock sysvar), seconds.
    ///
    /// # Errors
    ///
    /// `KeeperError::Chain` if the clock cannot be read.
    fn now(&self) -> Result<i64, KeeperError>;

    /// The owner and data of `account`, or `None` if it does not exist.
    ///
    /// # Errors
    ///
    /// `KeeperError::Chain` if the account cannot be read.
    fn account(&self, account: &Address) -> Result<Option<(Address, Vec<u8>)>, KeeperError>;

    /// Makes `print` available as a `PriceUpdateV2` of `feed` (in production: post the Hermes
    /// update through the Pyth receiver) and calls `sample` on `window` with it.
    ///
    /// # Errors
    ///
    /// `KeeperError::Chain` if the transaction fails.
    fn post_sample(
        &mut self,
        window: &Address,
        feed: &[u8; 32],
        print: &PrintMsg,
    ) -> Result<(), KeeperError>;

    /// Calls `finalize` on `window`.
    ///
    /// # Errors
    ///
    /// `KeeperError::Chain` if the transaction fails.
    fn finalize(&mut self, window: &Address) -> Result<(), KeeperError>;
}

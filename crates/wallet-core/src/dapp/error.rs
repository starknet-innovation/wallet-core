//! The wallet API's error objects (`wallet_rpc.json`; the extension's
//! `packages/foundation/wallet-protocol/src/provider/rpc-errors.ts`). Every refusal a dapp sees
//! is one of these, by number, never a raw string.
//!
//! The extension maps its background's error strings onto them (`errorForMessage`); the core
//! returns the code itself, so that mapping has nothing to port.

/// A wallet API error: its code, the spec's message, and an optional `data` string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletRpcError {
    pub code: i64,
    pub message: &'static str,
    pub data: Option<String>,
}

macro_rules! codes {
    ($($(#[$doc:meta])* $name:ident = $code:literal, $message:literal;)*) => {
        impl WalletRpcError {
            $(
                $(#[$doc])*
                pub fn $name() -> Self {
                    Self { code: $code, message: $message, data: None }
                }
            )*
        }
    };
}

codes! {
    /// NOT_ERC20 (111).
    not_erc20 = 111, "The asset is not a valid ERC20 token";
    /// UNLISTED_NETWORK (112).
    unlisted_network = 112, "The requested network is not listed";
    /// USER_REFUSED_OP (113): the person declined.
    user_refused_op = 113, "The user refused the operation";
    /// INVALID_REQUEST_PAYLOAD (114): the request or its params did not parse.
    invalid_request_payload = 114, "The request payload is invalid";
    /// ACCOUNT_ALREADY_DEPLOYED (115).
    account_already_deployed = 115, "The account is already deployed";
    /// DEPLOYMENT_DATA_NOT_AVAILABLE (116).
    deployment_data_not_available = 116, "Account deployment data is not available";
    /// CHAIN_ID_NOT_SUPPORTED (117).
    chain_id_not_supported = 117, "The chain id is not supported";
    /// NOT_REGISTERED (118).
    not_registered = 118, "The account is not registered";
    /// INSUFFICIENT_PRIVATE_BALANCE (119).
    insufficient_private_balance = 119, "Insufficient private balance";
    /// PRIVACY_LEAK (120).
    privacy_leak = 120, "The operation would leak private information";
    /// API_VERSION_NOT_SUPPORTED (162).
    api_version_not_supported = 162, "The requested API version is not supported";
    /// UNKNOWN_ERROR (163): anything else.
    unknown_error = 163, "The wallet request failed";
}

impl WalletRpcError {
    /// The same error with a `data` string, for a refusal that names its rule.
    pub fn with_data(self, data: impl Into<String>) -> Self {
        Self {
            data: Some(data.into()),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `error-codes.test.ts`'s table of spec codes and messages.
    #[test]
    fn codes_and_messages_are_the_spec_ones() {
        let table = [
            (WalletRpcError::not_erc20(), 111),
            (WalletRpcError::unlisted_network(), 112),
            (WalletRpcError::user_refused_op(), 113),
            (WalletRpcError::invalid_request_payload(), 114),
            (WalletRpcError::account_already_deployed(), 115),
            (WalletRpcError::deployment_data_not_available(), 116),
            (WalletRpcError::chain_id_not_supported(), 117),
            (WalletRpcError::not_registered(), 118),
            (WalletRpcError::insufficient_private_balance(), 119),
            (WalletRpcError::privacy_leak(), 120),
            (WalletRpcError::api_version_not_supported(), 162),
            (WalletRpcError::unknown_error(), 163),
        ];
        for (error, code) in table {
            assert_eq!(error.code, code);
            assert!(!error.message.is_empty());
            assert_eq!(error.data, None);
        }
        assert_eq!(
            WalletRpcError::user_refused_op().message,
            "The user refused the operation"
        );
    }

    #[test]
    fn data_rides_beside_the_spec_message() {
        let error = WalletRpcError::invalid_request_payload().with_data("rule");
        assert_eq!(error.code, 114);
        assert_eq!(error.message, "The request payload is invalid");
        assert_eq!(error.data.as_deref(), Some("rule"));
    }
}

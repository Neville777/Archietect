/// A pre-existing, unrelated payment-gateway connector — nothing to do with
/// job dispatch. Minimal reproduction of a real collision found live:
/// `crates/payment_gateway_connector/src/executor.rs`.
pub struct PaymentGatewayExecutor {
    pub api_key: String,
}

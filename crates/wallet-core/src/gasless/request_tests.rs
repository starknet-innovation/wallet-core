//! `gasless-route.test.ts`'s build-request and supported-token cases, and the session's gates.

use super::*;
use crate::gasless::tests::{call, felt_of, v2_classes, MAINNET_URL, POLICY};
use crate::gasless::{GaslessPolicy, GaslessRoute};
use crate::js::json;

const USER: &str = "0x213c67ed78bc280887234fe5ed5e77272465317978ae86c25a71531d9332a2d";
const USDC: &str = "0x53c91253bc9682c04929ca02ed00b3e423f6710d2ee7e0d5ebb06f3ecf368a8";
const ARGENT_04: &str = "0x36078334509b514626504edc9fb252328d1a240e4e948bef8d0c08dff45927f";

fn account(network: NetworkId, address: Felt, deployed: bool) -> GaslessAccount<'static> {
    GaslessAccount {
        network,
        address,
        family: AccountFamily::Argent,
        class_hash: felt_of(ARGENT_04),
        deployed,
        deployment: None,
    }
}

fn body(request: &HttpRequest) -> serde_json::Value {
    serde_json::from_str(request.body.as_deref().unwrap()).unwrap()
}

#[test]
fn the_build_sends_default_mode_only_with_0x_felts_no_tip_and_a_numeric_ten_minute_window() {
    let calls = [call(USER, "transfer", &["0x1", "0x2"])];
    let session = GaslessSession::open(
        &POLICY,
        &account(NetworkId::Mainnet, felt_of(USER), true),
        &calls,
        &v2_classes(),
    )
    .unwrap();
    let now = 1_700_000_000;
    let request = session.build_request(&felt_of(USDC), now).unwrap();
    assert_eq!(request.url, MAINNET_URL);
    let text = request.body.clone().unwrap();
    assert_eq!(
        text,
        format!(
            r#"{{"id":1,"jsonrpc":"2.0","method":"paymaster_buildTransaction","params":{{"parameters":{{"fee_mode":{{"gas_token":"{USDC}","mode":"default"}},"time_bounds":{{"execute_after":1,"execute_before":{}}},"version":"0x1"}},"transaction":{{"invoke":{{"calls":[{{"calldata":["0x1","0x2"],"selector":"0x83afd3f4caedc6eebf44246fe54e38c95e3179a5ec9ea81740eca5b482d12e","to":"{USER}"}}],"user_address":"{USER}"}},"type":"invoke"}}}}}}"#,
            now + 600
        )
    );
    assert!(!text.contains("sponsored") && !text.contains("userAddress"));
    assert_eq!(
        session.build_request(&felt_of(USDC), (1 << 53) - 600),
        Err(GaslessCode::Unavailable)
    );
}

#[test]
fn a_deployment_uses_a_numeric_version_and_must_deploy_the_invoking_account() {
    let deployment = Deployment {
        class_hash: felt_of("0x4"),
        salt: felt_of("0x3"),
        constructor_calldata: vec![felt_of("0x5")],
    };
    let address = deployment.address().unwrap();
    const ROUTES: [GaslessRoute; 1] = [GaslessRoute {
        network: NetworkId::Sepolia,
        url: "https://gateway.example/sepolia",
        enabled: true,
        forwarder: Some(Felt::from_hex_unchecked("0x9")),
    }];
    let sepolia = GaslessPolicy {
        routes: &ROUTES,
        ..POLICY
    };
    let calls = [call(USER, "transfer", &[])];
    let undeployed = GaslessAccount {
        deployment: Some(&deployment),
        ..account(NetworkId::Sepolia, address, false)
    };
    let session = GaslessSession::open(&sepolia, &undeployed, &calls, &v2_classes()).unwrap();
    let params = &body(&session.build_request(&felt_of(USDC), 1).unwrap())["params"];
    assert_eq!(params["transaction"]["type"], "deploy_and_invoke");
    assert_eq!(params["transaction"]["deployment"]["version"], 1);
    assert_eq!(
        params["transaction"]["deployment"]["address"],
        felt::canonical(&address)
    );
    let elsewhere = GaslessAccount {
        address: felt_of(USER),
        ..undeployed
    };
    assert_eq!(
        GaslessSession::open(&sepolia, &elsewhere, &calls, &v2_classes()),
        Err(GaslessCode::Unavailable)
    );
}

#[test]
fn the_gates_refuse_in_the_extensions_order() {
    let calls = [call(USDC, "transfer", &[USER, "0x1", "0x0"])];
    let open = |account: &GaslessAccount<'_>| {
        GaslessSession::open(&POLICY, account, &calls, &v2_classes()).err()
    };
    let user = felt_of(USER);
    assert_eq!(
        open(&account(NetworkId::Sepolia, user, true)),
        Some(GaslessCode::Unavailable)
    );
    let braavos = GaslessAccount {
        family: AccountFamily::Braavos,
        ..account(NetworkId::Mainnet, user, true)
    };
    assert_eq!(open(&braavos), Some(GaslessCode::V2Unsupported));
    assert_eq!(
        open(&account(NetworkId::Mainnet, user, false)),
        Some(GaslessCode::ScreeningSubjectUnverifiable)
    );
    assert_eq!(
        GaslessSession::open(
            &POLICY,
            &account(NetworkId::Mainnet, user, true),
            &[],
            &v2_classes()
        )
        .err(),
        Some(GaslessCode::Unavailable)
    );
}

#[test]
fn supported_tokens_keep_one_row_per_address_and_refuse_two_decimal_counts() {
    let parse = |text: &str| read_supported_tokens(&json::parse(text).unwrap());
    let row = format!(r#"{{"decimals":6,"token_address":"{USDC}"}}"#);
    let padded = format!(r#"{{"decimals":6,"token_address":"0x0{}"}}"#, &USDC[2..]);
    assert_eq!(
        parse(&format!("[{row},{row},{padded}]")),
        Some(vec![ListedToken {
            address: felt_of(USDC),
            decimals: 6
        }])
    );
    let eighteen = format!(r#"{{"decimals":18,"token_address":"{USDC}"}}"#);
    assert_eq!(parse(&format!("[{row},{eighteen}]")), None);
    let priced = format!(r#"[{{"decimals":6,"price_in_strk":"0x5","token_address":"{USDC}"}}]"#);
    assert_eq!(parse(&priced).map(|t| t.len()), Some(1));
    assert_eq!(parse(r#"[{"decimals":6,"token_address":"0x0"}]"#), None);
    assert_eq!(
        parse(&format!(r#"[{{"decimals":6.5,"token_address":"{USDC}"}}]"#)),
        None
    );
    assert_eq!(
        parse(&format!(
            r#"[{{"decimals":6,"symbol":"X","token_address":"{USDC}"}}]"#
        )),
        None
    );
}

#[test]
fn only_true_is_available_and_only_a_non_zero_felt_is_a_transaction_hash() {
    assert!(read_available(&JsValue::Bool(true)));
    assert!(!read_available(&JsValue::String("true".into())));
    let hash = |text: &str| read_transaction_hash(&json::parse(text).unwrap());
    assert_eq!(hash(r#"{"transaction_hash":"0x3"}"#), Some(felt_of("0x3")));
    assert_eq!(hash(r#""0x3""#), Some(felt_of("0x3")));
    assert_eq!(hash(r#"{"transaction_hash":"0x0"}"#), None);
    assert_eq!(hash(r#"{"transaction_hash":"3"}"#), None);
}

#[test]
fn a_quote_must_be_the_build_asked_for_with_every_fee_field_a_felt() {
    let calls = [call(USDC, "transfer", &[USER, "0x1", "0x0"])];
    let session = GaslessSession::open(
        &POLICY,
        &account(NetworkId::Mainnet, felt_of(USER), true),
        &calls,
        &v2_classes(),
    )
    .unwrap();
    let fee = r#"{"estimated_fee_in_gas_token":"0x1","estimated_fee_in_strk":"0x1","gas_token_price_in_strk":"0x1","suggested_max_fee_in_gas_token":"0x3e8","suggested_max_fee_in_strk":"0x1"}"#;
    let quote = |kind: &str, extra: &str| {
        json::parse(&format!(
            r#"{{"fee":{fee},"parameters":{{}},"type":"{kind}","typed_data":{{}}{extra}}}"#
        ))
        .unwrap()
    };
    let read = |value: &JsValue| session.read_quote(value).map(|(max, _)| max);
    assert_eq!(read(&quote("invoke", "")), Some(U256::from_u128(1000)));
    assert_eq!(read(&quote("deploy_and_invoke", "")), None);
    assert_eq!(read(&quote("invoke", r#","deployment":{}"#)), None);
    assert_eq!(read(&quote("invoke", r#","extra":1"#)), None);
    let decimal = json::parse(&quote("invoke", "").to_json().replace("0x3e8", "1000")).unwrap();
    assert_eq!(read(&decimal), None);
}

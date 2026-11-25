//! Ported from the extension's `tests/background/routing/{background-messages,
//! informational-perimeter}.test.ts`, `settings/background-blocklist-enforcement.test.ts`,
//! `portfolio/background-discovery-messages.test.ts` and `runtime/grant-read-session-races
//! .test.ts`: the gates in the order the extension runs them.

use super::*;

const SITE: &str = "https://app.example";
const MAINNET: &str = "0x534e5f4d41494e";
const VERSIONS: Versions = Versions {
    specs: &["0.10.0"],
    wallet_api: &["0.10.5"],
};

fn account(address: &str) -> DappAccount {
    DappAccount {
        id: format!("SN_MAIN:{address}"),
        address: address.into(),
        network_id: "SN_MAIN".into(),
    }
}

fn grant(origin: &str, ids: &[&str]) -> OriginGrant {
    OriginGrant {
        account_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
        origin: origin.into(),
    }
}

fn site<'a>(blocked: bool, unlocked: Option<Unlocked<'a>>) -> Site<'a> {
    Site {
        origin: SITE,
        blocked,
        unlocked,
        chain_id: MAINNET,
    }
}

fn ask(request: Request, site: &Site<'_>) -> Result<Answer, WalletRpcError> {
    answer(&request, site, &VERSIONS).expect("a connect-slice method")
}

fn result(text: &str) -> Result<Answer, WalletRpcError> {
    Ok(Answer::Result(crate::js::json::parse(text).unwrap()))
}

fn accounts() -> Request {
    Request::RequestAccounts { silent: false }
}

// "reads only origin-granted accounts"; "creates pending approvals for ungranted dapp account
// requests"; "returns no accounts without prompting for silent ungranted requests".
#[test]
fn a_granted_site_sees_the_active_account_and_any_other_is_asked() {
    let active = account("0x1");
    let grants = [grant(SITE, &["SN_MAIN:0x1"])];
    let connected = site(
        false,
        Some(Unlocked {
            grants: &grants,
            active: Some(&active),
        }),
    );
    assert_eq!(ask(accounts(), &connected), result(r#"["0x1"]"#));
    assert_eq!(
        ask(Request::RequestAccounts { silent: true }, &connected),
        result(r#"["0x1"]"#)
    );

    let none: [OriginGrant; 0] = [];
    let stranger = site(
        false,
        Some(Unlocked {
            grants: &none,
            active: Some(&active),
        }),
    );
    assert_eq!(ask(accounts(), &stranger), Ok(Answer::Connect));
    assert_eq!(
        ask(Request::RequestAccounts { silent: true }, &stranger),
        result("[]")
    );
}

// "re-prompts a reconnecting origin whose grant misses the active account": a grant covers the
// accounts it names, and a page only ever sees the active one.
#[test]
fn a_grant_for_another_account_asks_again() {
    let active = account("0x2");
    let grants = [grant(SITE, &["SN_MAIN:0x1"])];
    let moved = site(
        false,
        Some(Unlocked {
            grants: &grants,
            active: Some(&active),
        }),
    );
    assert_eq!(moved.granted(), None);
    assert_eq!(ask(accounts(), &moved), Ok(Answer::Connect));
    assert_eq!(ask(Request::GetPermissions, &moved), result("[]"));
}

// "wallet.requestAccounts queues a connection approval when a lock lands mid-read, as it does
// while locked": locked looks unconnected, never like an error.
#[test]
fn a_locked_wallet_answers_as_an_unconnected_one() {
    let locked = site(false, None);
    assert_eq!(ask(accounts(), &locked), Ok(Answer::Connect));
    assert_eq!(
        ask(Request::RequestAccounts { silent: true }, &locked),
        result("[]")
    );
    assert_eq!(ask(Request::GetPermissions, &locked), result("[]"));
    assert_eq!(
        ask(Request::RequestChainId, &locked),
        result(r#""0x534e5f4d41494e""#)
    );
}

// `blocklist enforced beyond connect (A-M2)` and `informational dapp surfaces`: a blocked origin
// holding an old grant sees no account and no permission, and every other read is refused.
#[test]
fn a_blocked_site_learns_nothing() {
    let active = account("0x1");
    let grants = [grant(SITE, &["SN_MAIN:0x1"])];
    let blocked = site(
        true,
        Some(Unlocked {
            grants: &grants,
            active: Some(&active),
        }),
    );
    assert_eq!(blocked.granted(), None);
    assert_eq!(ask(Request::GetPermissions, &blocked), result("[]"));
    for request in [
        accounts(),
        Request::RequestAccounts { silent: true },
        Request::RequestChainId,
        Request::SupportedSpecs { api_version: None },
        Request::SupportedWalletApi { api_version: None },
    ] {
        assert_eq!(
            ask(request.clone(), &blocked),
            Err(WalletRpcError::unknown_error()),
            "{request:?}"
        );
    }
    assert_eq!(
        request_accounts(&blocked, true),
        Err(WalletRpcError::unknown_error())
    );
}

#[test]
fn a_connected_site_holds_the_accounts_permission() {
    let active = account("0x1");
    let grants = [grant(SITE, &["SN_MAIN:0x1"])];
    let connected = site(
        false,
        Some(Unlocked {
            grants: &grants,
            active: Some(&active),
        }),
    );
    assert_eq!(
        ask(Request::GetPermissions, &connected),
        result(r#"["accounts"]"#)
    );
}

// `background discovery messages`: the lists, and `api_version` negotiated against the wallet
// API versions for both methods.
#[test]
fn discovery_negotiates_the_api_version() {
    let open = site(false, None);
    let specs = |api_version: Option<&str>| Request::SupportedSpecs {
        api_version: api_version.map(str::to_owned),
    };
    let api = |api_version: Option<&str>| Request::SupportedWalletApi {
        api_version: api_version.map(str::to_owned),
    };
    assert_eq!(ask(specs(None), &open), result(r#"["0.10.0"]"#));
    assert_eq!(ask(api(None), &open), result(r#"["0.10.5"]"#));
    assert_eq!(ask(specs(Some("0.10.5")), &open), result(r#"["0.10.0"]"#));
    assert_eq!(ask(api(Some("0.10.5")), &open), result(r#"["0.10.5"]"#));
    for request in [specs(Some("0.10.0")), api(Some("0.9"))] {
        assert_eq!(
            ask(request, &open),
            Err(WalletRpcError::api_version_not_supported())
        );
    }
}

#[test]
fn other_methods_are_not_this_slices() {
    let open = site(false, None);
    for request in [
        Request::DeploymentData { api_version: None },
        Request::SwitchChain {
            chain_id: MAINNET.into(),
        },
        Request::AddInvoke { calls: Vec::new() },
    ] {
        assert_eq!(answer(&request, &open, &VERSIONS), None, "{request:?}");
    }
}

// "approves pending dapp account requests after unlock": the grant is added for the active
// account, which the waiting pages then see.
#[test]
fn approving_grants_the_reviewed_active_account() {
    let active = account("0x2");
    let grants = [
        grant("https://other.example", &["SN_MAIN:0x1"]),
        grant(SITE, &["SN_MAIN:0x1"]),
    ];
    let unlocked = site(
        false,
        Some(Unlocked {
            grants: &grants,
            active: Some(&active),
        }),
    );
    let (written, seen) = approve_connect(&unlocked, Some("SN_MAIN:0x2")).unwrap();
    assert_eq!(seen, &active);
    assert_eq!(
        written,
        vec![
            grant("https://other.example", &["SN_MAIN:0x1"]),
            grant(SITE, &["SN_MAIN:0x1", "SN_MAIN:0x2"]),
        ]
    );
    // With no echo (Bramble's page asks with no card), the active account is granted.
    assert!(approve_connect(&unlocked, None).is_ok());
}

// `ACCOUNT_CHANGED_REFUSAL`, a blocked origin at decision time, a locked wallet: nothing written.
#[test]
fn approving_refuses_a_moved_account_a_blocked_site_and_a_lock() {
    let active = account("0x2");
    let none: [OriginGrant; 0] = [];
    let unlocked = Unlocked {
        grants: &none,
        active: Some(&active),
    };
    let refused = Err(WalletRpcError::unknown_error());
    assert_eq!(
        approve_connect(&site(false, Some(unlocked)), Some("SN_MAIN:0x1")),
        refused
    );
    assert_eq!(approve_connect(&site(true, Some(unlocked)), None), refused);
    assert_eq!(approve_connect(&site(false, None), None), refused);
    let no_active = Unlocked {
        grants: &none,
        active: None,
    };
    assert_eq!(
        approve_connect(&site(false, Some(no_active)), None),
        refused
    );
}

#[test]
fn denying_is_the_persons_refusal() {
    assert_eq!(deny_connect(), WalletRpcError::user_refused_op());
}

// `accountsViewPayload`: an address and a network, never the vault's id.
#[test]
fn the_accounts_view_carries_no_id() {
    let active = account("0x1");
    assert_eq!(
        accounts_view(&[&active]).to_json(),
        r#"{"accounts":[{"address":"0x1","networkId":"SN_MAIN"}],"ok":true}"#
    );
    assert_eq!(accounts_view(&[]).to_json(), r#"{"accounts":[],"ok":true}"#);
}

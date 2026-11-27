//! Live-network helpers for the opt-in suites (`mainnet`, `sepolia_dataset`, `privacy_mainnet`,
//! `strk20_live_smoke`). The HTTP helpers shell out to `curl` and exist only in tests, so the core
//! stays sans-IO. Read-only: the scan sends `starknet_chainId`, `starknet_getClassHashAt` and
//! ERC-20 `balanceOf`, never a write.

use std::io::Write;
use std::process::{Command, Stdio};

use bramble_core::{
    rpc_chain_id_request, rpc_verify_chain_id, supported_networks, NetworkId, RecoveryCandidate,
    RpcResponse, WalletCore,
};

/// Fails the test when a CI marker is set: live suites hold real phrases and never run there.
pub fn refuse_ci() {
    for marker in [
        "CI",
        "GITHUB_ACTIONS",
        "TEST_RUNNER_CI",
        "TEST_RUNNER_GITHUB_ACTIONS",
    ] {
        assert!(
            std::env::var_os(marker).is_none(),
            "live wallet checks never run in CI ({marker} is set)"
        );
    }
}

/// Compares addresses numerically: a configured address may carry leading zeros.
pub fn canonical(value: &str) -> String {
    value
        .trim_start_matches("0x")
        .trim_start_matches('0')
        .to_lowercase()
}

/// The `curl` that POSTs a JSON body read from stdin to `url`: HTTPS only, no redirects, at most
/// `limit` bytes back, `timeout_ms` in all, and the HTTP status after a last newline. The body is
/// never an argument (it may hold a viewing key), and nothing is echoed: errors go nowhere.
pub fn curl_post(url: &str, limit: u64, timeout_ms: u64) -> Command {
    let mut curl = Command::new("curl");
    let timeout = format!("{}.{:03}", timeout_ms / 1000, timeout_ms % 1000);
    curl.args(["-sS", "--proto", "=https", "--max-redirs", "0"])
        .args(["--max-time", &timeout])
        .args(["--max-filesize", &limit.to_string()])
        .args(["-H", "Content-Type: application/json"])
        .args(["--data-binary", "@-", "-w", "\n%{http_code}", url])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    curl
}

/// POSTs `body` through [`curl_post`]; `(status, reply)`, or `None` on a transport failure, a
/// timeout, or a reply over `limit`.
pub fn post_json(url: &str, body: &[u8], limit: u64, timeout_ms: u64) -> Option<(u16, Vec<u8>)> {
    let mut child = curl_post(url, limit, timeout_ms).spawn().ok()?;
    child.stdin.take()?.write_all(body).ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    let split = output.stdout.iter().rposition(|b| *b == b'\n')?;
    let status = std::str::from_utf8(&output.stdout[split + 1..])
        .ok()?
        .parse()
        .ok()?;
    let reply = output.stdout[..split].to_vec();
    (reply.len() as u64 <= limit).then_some((status, reply))
}

pub fn post(url: &str, body: &str) -> String {
    let output = Command::new("curl")
        .args(["-sS", "--max-time", "20", "-X", "POST"])
        .args([
            "-H",
            "content-type: application/json",
            "--data-binary",
            body,
            url,
        ])
        .output()
        .expect("curl runs");
    assert!(output.status.success(), "request failed");
    String::from_utf8(output.stdout).expect("response is UTF-8")
}

/// The first bundled endpoint that proves `network`'s chain id.
pub fn endpoint(network: NetworkId) -> String {
    supported_networks()
        .into_iter()
        .find(|info| info.network == network)
        .unwrap()
        .rpc_urls
        .into_iter()
        .find(|url| {
            rpc_verify_chain_id(post(url, &rpc_chain_id_request(1)), 1, network).unwrap_or(false)
        })
        .expect("a bundled endpoint proves the chain")
}

/// Runs the default discovery scan to completion over `url` and returns its candidates. A batch
/// the core pauses on (rate limit, node error) is re-issued after a short wait, up to five times.
pub fn scan(core: &WalletCore, network: NetworkId, url: &str) -> Vec<RecoveryCandidate> {
    let mut retries = 0;
    let mut step = core.recovery_scan_next(network, None).unwrap();
    while !step.complete {
        while !step.requests.is_empty() {
            let responses = step
                .requests
                .iter()
                .map(|request| RpcResponse {
                    id: request.id,
                    body: post(url, &request.body),
                })
                .collect();
            match core.recovery_scan_submit(responses) {
                Ok(next) => step = next,
                Err(error) => {
                    retries += 1;
                    assert!(retries <= 5, "discovery kept pausing: {error:?}");
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    step = core.recovery_scan_next(network, None).unwrap();
                }
            }
        }
        if !step.complete {
            step = core.recovery_scan_next(network, None).unwrap();
        }
    }
    step.candidates
}

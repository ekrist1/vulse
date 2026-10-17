//! Client ID Metadata Documents: an MCP client may use the https URL of a JSON
//! document that describes it as its `client_id`, instead of registering.
//! Vulse reads the document from public addresses only, connects to the
//! address it checked, and never follows redirects.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use serde::Deserialize;

use crate::oauth::{check_client_metadata, split_uri};

const MAX_BYTES: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(5);

pub struct Metadata {
    pub name: String,
    pub redirect_uris: Vec<String>,
}

pub fn is_metadata_url(client_id: &str) -> bool {
    client_id.starts_with("https://")
}

/// The host of a metadata document URL, shown to users as who vouches for
/// the client's name.
pub fn domain(client_id: &str) -> Option<String> {
    is_metadata_url(client_id)
        .then(|| split_uri(client_id).map(|(_, host, _, _)| host.to_ascii_lowercase()))
        .flatten()
}

/// The host and port to fetch, for an https URL with a path and a domain name.
fn target(url: &str) -> Result<(String, u16), String> {
    let invalid = || "the client_id is not a usable https URL".to_string();
    if url.len() > 2000 || url.contains('#') || url.chars().any(|c| c.is_whitespace()) {
        return Err(invalid());
    }
    let (scheme, host, port, path) = split_uri(url).ok_or_else(invalid)?;
    if scheme != "https" || host.is_empty() || !path.starts_with('/') || path == "/" {
        return Err(invalid());
    }
    if host.parse::<IpAddr>().is_ok() || host.contains(':') {
        return Err("the client_id must use a domain name, not an IP address".into());
    }
    let port = match port {
        Some(port) => port.parse().map_err(|_| invalid())?,
        None => 443,
    };
    Ok((host.to_ascii_lowercase(), port))
}

/// Addresses on the public internet: not this machine, private networks,
/// link-local, shared, multicast, documentation or translation ranges.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || a == 0
                || a >= 240
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 198 && (b == 18 || b == 19)))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let [first, second, ..] = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || (first == 0x2001 && (second == 0x0db8 || second == 0))
                || first == 0x2002
                || (first == 0x0064 && second == 0xff9b))
        }
    }
}

/// Reads the document at `url`.
pub async fn fetch(url: &str) -> Result<Vec<u8>, String> {
    #[cfg(test)]
    if let Some(document) = test_documents().lock().unwrap().get(url) {
        return Ok(document.clone().into_bytes());
    }
    let (host, port) = target(url)?;
    let unreachable = || format!("could not read the app's details from {host}");
    let address: SocketAddr = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|_| unreachable())?
        .find(|address| is_public(address.ip()))
        .ok_or_else(|| format!("{host} is not a public address"))?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
        .no_proxy()
        .resolve(&host, address)
        .build()
        .map_err(|_| unreachable())?;
    let mut response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|_| unreachable())?;
    if !response.status().is_success() {
        return Err(format!(
            "{host} answered {} for the app's details",
            response.status().as_u16()
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unreachable())? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BYTES {
            return Err("the app's details are too large".into());
        }
    }
    Ok(body)
}

#[derive(Deserialize)]
struct Document {
    client_id: String,
    #[serde(default)]
    client_name: Option<String>,
    #[serde(default)]
    redirect_uris: Vec<String>,
    #[serde(default)]
    token_endpoint_auth_method: Option<String>,
    #[serde(default)]
    grant_types: Option<Vec<String>>,
    #[serde(default)]
    response_types: Option<Vec<String>>,
}

/// Checks that the document at `url` describes a public client at that URL.
pub fn parse(url: &str, body: &[u8]) -> Result<Metadata, String> {
    let host = target(url)?.0;
    let document: Document = serde_json::from_slice(body)
        .map_err(|_| format!("the app's details at {host} are not valid"))?;
    if document.client_id != url {
        return Err(format!(
            "the app's details at {host} name a different client_id"
        ));
    }
    if document
        .token_endpoint_auth_method
        .as_deref()
        .is_some_and(|method| method != "none")
    {
        return Err("only public apps that sign in with PKCE can connect".into());
    }
    check_client_metadata(
        &document.redirect_uris,
        document.grant_types.as_deref(),
        document.response_types.as_deref(),
    )
    .map_err(|(_, description)| description.to_string())?;
    let name = document
        .client_name
        .map(|name| name.trim().chars().take(100).collect::<String>())
        .filter(|name| !name.is_empty())
        .unwrap_or(host);
    Ok(Metadata {
        name,
        redirect_uris: document.redirect_uris,
    })
}

/// Documents `fetch` returns in tests, by URL, instead of reading the network.
#[cfg(test)]
pub fn test_documents() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static DOCUMENTS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, String>>,
    > = std::sync::OnceLock::new();
    DOCUMENTS.get_or_init(Default::default)
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn only_public_addresses_are_fetched() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "2002:a00:1::",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "160.79.104.10", "2606:4700::1111"] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[tokio::test]
    async fn names_that_resolve_to_this_machine_are_not_fetched() {
        let err = fetch("https://localhost/client.json").await.unwrap_err();
        assert_eq!(err, "localhost is not a public address");
    }

    #[test]
    fn metadata_urls_need_a_domain_and_a_path() {
        assert_eq!(
            target("https://claude.ai/oauth/mcp-client").unwrap(),
            ("claude.ai".into(), 443)
        );
        assert_eq!(
            target("https://app.example:8443/client.json").unwrap().1,
            8443
        );
        for url in [
            "https://claude.ai",
            "https://claude.ai/",
            "http://claude.ai/client",
            "https://10.0.0.1/client",
            "https://[::1]/client",
            "https://user@claude.ai/client",
            "https://claude.ai/client#x",
        ] {
            assert!(target(url).is_err(), "{url}");
        }
    }

    #[test]
    fn documents_must_describe_a_public_client_at_their_url() {
        let url = "https://app.example/client.json";
        let document = |extra: &str| {
            format!(
                r#"{{"client_id":"{url}","client_name":"App","redirect_uris":["https://app.example/cb"]{extra}}}"#
            )
        };
        let parsed = parse(url, document("").as_bytes()).unwrap();
        assert_eq!(parsed.name, "App");
        assert_eq!(parsed.redirect_uris, ["https://app.example/cb"]);
        assert!(parse("https://other.example/client.json", document("").as_bytes()).is_err());
        assert!(
            parse(
                url,
                document(r#","token_endpoint_auth_method":"private_key_jwt""#).as_bytes()
            )
            .is_err()
        );
        assert!(
            parse(
                url,
                br#"{"client_id":"https://app.example/client.json","redirect_uris":["http://evil.example/cb"]}"#
            )
            .is_err()
        );
        assert!(parse(url, b"not json").is_err());
    }
}

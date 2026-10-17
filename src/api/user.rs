#![allow(clippy::missing_errors_doc)]

/// User profile API endpoints for the Fast.io REST API.
///
/// Maps to the endpoints documented in `/current/user/`.
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, USER_AGENT};
use reqwest::multipart;
use serde_json::Value;

use crate::client::ApiClient;
use crate::error::CliError;

/// User-Agent string for user asset requests.
const USER_ASSET_USER_AGENT: &str = concat!("fastio-cli/", env!("CARGO_PKG_VERSION"));

/// Timeout for user asset upload requests.
const ASSET_UPLOAD_TIMEOUT_SECS: u64 = 120;

/// Connect timeout for user asset requests.
const ASSET_CONNECT_TIMEOUT_SECS: u64 = 30;

/// Get the current user's profile details.
///
/// `GET /user/me/details/`
pub async fn get_me(client: &ApiClient) -> Result<Value, CliError> {
    client.get("/user/me/details/").await
}

/// Get a user's details by ID.
///
/// `GET /user/{user_id}/details/`
pub async fn get_user_by_id(client: &ApiClient, user_id: &str) -> Result<Value, CliError> {
    let path = format!("/user/{}/details/", urlencoding::encode(user_id));
    client.get(&path).await
}

/// Fields for a `POST /user/update/` profile update.
///
/// Every field is optional; only the ones set are sent. This struct
/// deliberately does NOT derive `Debug` because it carries `password` and
/// `current_password` secrets — they must never appear in Debug output.
///
/// Per `auth.txt`, changing `email_address` or `password` on an account that
/// already has a password REQUIRES `current_password`; an `email_address`
/// change does not take effect until confirmed via
/// [`crate::api::auth::email_change_confirm`]. Setting `phone_country` /
/// `phone_number` requires 2FA to be disabled first, and both must be sent
/// together.
///
/// Construct with struct-literal syntax plus `..Default::default()` so new
/// fields can be added without breaking callers. It is deliberately not
/// `#[non_exhaustive]` — that attribute would block the struct expression from
/// the separate `fastio` binary crate.
#[derive(Default)]
pub struct UserUpdate<'a> {
    /// First name.
    pub first_name: Option<&'a str>,
    /// Last name.
    pub last_name: Option<&'a str>,
    /// New email address (triggers a confirm-by-email change flow).
    pub email_address: Option<&'a str>,
    /// New password.
    pub password: Option<&'a str>,
    /// Current password proof (required to change email or password on an
    /// account that already has one).
    pub current_password: Option<&'a str>,
    /// Numeric phone country code (e.g. `"1"` for US).
    pub phone_country: Option<&'a str>,
    /// Numeric phone number.
    pub phone_number: Option<&'a str>,
}

/// Update the current user's profile.
///
/// `POST /user/update/` with form-encoded body.
pub async fn update_user(client: &ApiClient, update: &UserUpdate<'_>) -> Result<Value, CliError> {
    let mut form = HashMap::new();
    if let Some(v) = update.first_name {
        form.insert("first_name".to_owned(), v.to_owned());
    }
    if let Some(v) = update.last_name {
        form.insert("last_name".to_owned(), v.to_owned());
    }
    if let Some(v) = update.email_address {
        form.insert("email_address".to_owned(), v.to_owned());
    }
    if let Some(v) = update.password {
        form.insert("password".to_owned(), v.to_owned());
    }
    if let Some(v) = update.current_password {
        form.insert("current_password".to_owned(), v.to_owned());
    }
    if let Some(v) = update.phone_country {
        form.insert("phone_country".to_owned(), v.to_owned());
    }
    if let Some(v) = update.phone_number {
        form.insert("phone_number".to_owned(), v.to_owned());
    }
    client.post("/user/update/", &form).await
}

/// Upload a user asset (e.g. avatar) via multipart form data.
///
/// `POST /user/{user_id}/assets/{asset_name}/`
///
/// This uses a raw reqwest client because the API expects `multipart/form-data`
/// with a binary file field.
pub async fn upload_user_asset(
    client: &ApiClient,
    user_id: &str,
    asset_name: &str,
    file_path: &str,
) -> Result<Value, CliError> {
    upload_entity_asset(client, "user", user_id, asset_name, file_path).await
}

/// Upload an asset to ANY entity that carries the asset route family.
///
/// The contract is byte-identical across entity types, so one implementation
/// serves them all. The route is uniform:
/// `POST /{entity_type}/{entity_id}/assets/{asset_name}/`, multipart, for
/// `user`, `org`, `workspace` and `share`. Only the entity segment ever
/// differs, which is why `delete_asset` is likewise generic.
pub async fn upload_entity_asset(
    client: &ApiClient,
    entity_type: &str,
    entity_id: &str,
    asset_name: &str,
    file_path: &str,
) -> Result<Value, CliError> {
    let token = client
        .get_token()
        .ok_or_else(|| CliError::Config("authentication required for asset upload".to_owned()))?
        .to_owned();
    let base = client.base_url();
    let url = format!(
        "{}{}",
        base.trim_end_matches('/'),
        crate::api::asset::asset_upload_path(entity_type, entity_id, asset_name),
    );

    let path = Path::new(file_path);
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("asset")
        .to_owned();
    let file_bytes = tokio::fs::read(path)
        .await
        .map_err(|e| CliError::Config(format!("failed to read file {file_path}: {e}")))?;

    let part = multipart::Part::bytes(file_bytes).file_name(file_name);
    let form = multipart::Form::new().part("file", part);

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(ASSET_UPLOAD_TIMEOUT_SECS))
        .connect_timeout(Duration::from_secs(ASSET_CONNECT_TIMEOUT_SECS))
        .build()
        .map_err(CliError::Http)?;

    let resp = http_client
        .post(&url)
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header(USER_AGENT, USER_ASSET_USER_AGENT)
        .multipart(form)
        .send()
        .await?;

    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| CliError::Http(crate::error::without_request_url(e)))?;

    if !status.is_success() {
        let msg = body
            .get("error")
            .and_then(|e| e.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("asset upload failed");
        return Err(CliError::Api(crate::error::ApiError {
            code: 0,
            error_code: None,
            message: msg.to_owned(),
            http_status: status.as_u16(),
            details: body.get("error").and_then(ApiClient::extract_error_details),
        }));
    }

    // Unwrap the API response envelope per convention.
    let result = body.get("result").and_then(|v| v.as_str());
    if result == Some("yes") {
        if let Some(inner) = body.get("response") {
            Ok(inner.clone())
        } else {
            Ok(body)
        }
    } else {
        let msg = body
            .get("error")
            .and_then(|e| e.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("asset upload failed");
        Err(CliError::Api(crate::error::ApiError {
            code: 0,
            error_code: None,
            message: msg.to_owned(),
            http_status: status.as_u16(),
            details: body.get("error").and_then(ApiClient::extract_error_details),
        }))
    }
}

/// Delete a user asset (e.g. avatar).
///
/// `DELETE /user/{user_id}/assets/{asset_name}/`
pub async fn delete_user_asset(
    client: &ApiClient,
    user_id: &str,
    asset_name: &str,
) -> Result<Value, CliError> {
    let path = format!(
        "/user/{}/assets/{}/",
        urlencoding::encode(user_id),
        urlencoding::encode(asset_name),
    );
    client.delete(&path).await
}

/// Get available user asset types.
///
/// `GET /user/assets/`
pub async fn get_asset_types(client: &ApiClient) -> Result<Value, CliError> {
    client.get("/user/assets/").await
}

/// List a user's assets.
///
/// `GET /user/{user_id}/assets/`
pub async fn list_user_assets(client: &ApiClient, user_id: &str) -> Result<Value, CliError> {
    let path = format!("/user/{}/assets/", urlencoding::encode(user_id));
    client.get(&path).await
}

/// Get the current user's available profiles.
///
/// `GET /user/available_profiles/`
pub async fn get_profiles(client: &ApiClient) -> Result<Value, CliError> {
    client.get("/user/available_profiles/").await
}

/// List user invitations.
///
/// `GET /user/invitations/list/`
pub async fn list_invitations(
    client: &ApiClient,
    invitation_key: Option<&str>,
) -> Result<Value, CliError> {
    if let Some(key) = invitation_key {
        let mut params = HashMap::new();
        params.insert("invitation_key".to_owned(), key.to_owned());
        client
            .get_with_params("/user/invitations/list/", &params)
            .await
    } else {
        client.get("/user/invitations/list/").await
    }
}

/// Get details of a specific invitation.
///
/// `GET /user/invitation/{invitation_id}/details/`
pub async fn get_invitation_details(
    client: &ApiClient,
    invitation_id: &str,
) -> Result<Value, CliError> {
    let path = format!(
        "/user/invitation/{}/details/",
        urlencoding::encode(invitation_id),
    );
    client.get(&path).await
}

/// Accept all pending invitations.
///
/// `POST /user/invitations/acceptall/`
pub async fn accept_all_invitations(
    client: &ApiClient,
    invitation_key: Option<&str>,
) -> Result<Value, CliError> {
    let mut form = HashMap::new();
    if let Some(key) = invitation_key {
        form.insert("invitation_key".to_owned(), key.to_owned());
    }
    client.post("/user/invitations/acceptall/", &form).await
}

/// Search users by query.
///
/// `GET /users/search/` — the query is sent as the `search` param (the server
/// reads `PostOrQueryInput('search')`; an old `query` key is silently ignored).
pub async fn search_users(client: &ApiClient, query: &str) -> Result<Value, CliError> {
    let mut params = HashMap::new();
    params.insert("search".to_owned(), query.to_owned());
    client.get_with_params("/users/search/", &params).await
}

/// Build the close-account form body (pure; unit-tested).
fn close_account_form(email_address: &str, dryrun: bool) -> HashMap<String, String> {
    let mut form = HashMap::new();
    form.insert("email_address".to_owned(), email_address.to_owned());
    if dryrun {
        form.insert("dryrun".to_owned(), "true".to_owned());
    }
    form
}

/// Close (soft-delete) the current user's account.
///
/// `POST /user/close/`
///
/// The required field is **`email_address`**, and it must match the account's
/// current address. A field named `confirmation` is NOT declared by the route,
/// and because the platform drops undeclared fields silently, sending that
/// instead leaves the server with a close request carrying **no required
/// parameter at all** — which is why the CLI asks for the email itself rather
/// than a generic "confirmation string".
///
/// `dryrun` checks eligibility WITHOUT closing the account — the only safe way
/// to rehearse an irreversible action.
pub async fn close_account(
    client: &ApiClient,
    email_address: &str,
    dryrun: bool,
) -> Result<Value, CliError> {
    let form = close_account_form(email_address, dryrun);
    client.post("/user/close/", &form).await
}

/// Check whether the user is authorized in their country.
///
/// `GET /user/me/allowed/`
pub async fn user_allowed(client: &ApiClient) -> Result<Value, CliError> {
    client.get("/user/me/allowed/").await
}

/// Check org creation eligibility / limits.
///
/// `GET /user/me/limits/orgs/`
pub async fn user_org_limits(client: &ApiClient) -> Result<Value, CliError> {
    client.get("/user/me/limits/orgs/").await
}

/// List the user's shares.
///
/// `GET /shares/all/`
pub async fn list_user_shares(client: &ApiClient) -> Result<Value, CliError> {
    client.get("/shares/all/").await
}

/// Enable or disable photo auto-sync from SSO providers.
///
/// `GET /user/me/autosync/{state}/`
///
/// State must be `"enable"` or `"disable"`.
pub async fn autosync(client: &ApiClient, state: &str) -> Result<Value, CliError> {
    let path = format!("/user/me/autosync/{}/", urlencoding::encode(state));
    client.get(&path).await
}

/// Read the binary content of a user asset.
///
/// `GET /user/{user_id}/assets/{asset_name}/read/`
///
/// Downloads the asset to the given output path. Returns the number of bytes written.
pub async fn read_user_asset(
    client: &ApiClient,
    user_id: &str,
    asset_name: &str,
    output_path: &Path,
) -> Result<u64, CliError> {
    let token = client
        .get_token()
        .ok_or_else(|| CliError::Config("authentication required for asset read".to_owned()))?
        .to_owned();
    let base = client.base_url();
    let url = format!(
        "{}/user/{}/assets/{}/read/",
        base.trim_end_matches('/'),
        urlencoding::encode(user_id),
        urlencoding::encode(asset_name),
    );

    let http_client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(ASSET_CONNECT_TIMEOUT_SECS))
        .build()
        .map_err(CliError::Http)?;

    let resp = http_client
        .get(&url)
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header(USER_AGENT, USER_ASSET_USER_AGENT)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(CliError::Api(crate::error::ApiError {
            code: 0,
            error_code: None,
            message: format!("asset read failed with HTTP {status}: {body}"),
            http_status: status,
            // A refused read answers with the JSON error envelope; keep its
            // enrichment so the reason-keyed hints can fire.
            details: serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v.get("error").and_then(ApiClient::extract_error_details)),
        }));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| CliError::Http(crate::error::without_request_url(e)))?;
    tokio::fs::write(output_path, &bytes)
        .await
        .map_err(|e| CliError::Config(format!("failed to write file: {e}")))?;

    Ok(bytes.len() as u64)
}

/// Get the user's support PIN and identity verification hash.
///
/// `GET /user/pin/`
pub async fn get_pin(client: &ApiClient) -> Result<Value, CliError> {
    client.get("/user/pin/").await
}

/// Validate a phone number and country code combination.
///
/// `GET /user/phone/{country_code}-{phone_number}/`
pub async fn validate_phone(
    client: &ApiClient,
    country_code: &str,
    phone_number: &str,
) -> Result<Value, CliError> {
    let path = format!(
        "/user/phone/{}-{}/",
        urlencoding::encode(country_code),
        urlencoding::encode(phone_number),
    );
    client.get(&path).await
}

#[cfg(test)]
mod tests {
    /// The close-account form must carry `email_address`, never
    /// `confirmation`.
    ///
    /// Per the published API docs, `email_address` is REQUIRED and "must match
    /// the user's current email address". A field called `confirmation` is not
    /// declared by the route, and the platform drops undeclared fields
    /// silently — so sending that instead leaves the server with an
    /// account-close request carrying no required parameter at all. Asserting
    /// the wrong key is ABSENT is the
    /// load-bearing half; a check that only looks for the right key would pass
    /// while both were sent.
    #[test]
    fn close_account_form_uses_email_address_not_confirmation() {
        let form = super::close_account_form("jane.doe@example.com", false);
        assert_eq!(
            form.get("email_address").map(String::as_str),
            Some("jane.doe@example.com")
        );
        assert!(
            !form.contains_key("confirmation"),
            "`confirmation` is not a parameter of this route: {form:?}"
        );
        assert!(
            !form.contains_key("dryrun"),
            "dryrun must be omitted unless asked for — a stray truthy value \
             would turn a real close into a no-op: {form:?}"
        );
    }

    /// `dryrun` rehearses an IRREVERSIBLE action, so it has to
    /// actually reach the wire when requested.
    #[test]
    fn close_account_dryrun_is_sent_when_requested() {
        let form = super::close_account_form("jane.doe@example.com", true);
        assert_eq!(form.get("dryrun").map(String::as_str), Some("true"));
        assert!(form.contains_key("email_address"));
    }

    /// The SA-refusal body: `error.params.reason` is `service_account_not_permitted`.
    const SA_DENIED: &[u8] = br#"{"result":"no","error":{"code":1680,"text":"Service accounts are not permitted to perform this action.","params":{"reason":"service_account_not_permitted"}}}"#;

    /// Serve ONE response with `status_line` and `body` after draining the full
    /// request (headers plus `Content-Length` body), so a multipart upload is
    /// not reset mid-write. Returns the bound `127.0.0.1:<port>`.
    async fn spawn_one_shot(status_line: &'static str, body: &'static [u8]) -> String {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr").to_string();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut req: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 4096];
                let mut want: Option<usize> = None;
                loop {
                    if want.is_some_and(|w| req.len() >= w) {
                        break;
                    }
                    match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            req.extend_from_slice(&chunk[..n]);
                            if want.is_none()
                                && let Some(pos) = req.windows(4).position(|w| w == b"\r\n\r\n")
                            {
                                let head =
                                    String::from_utf8_lossy(&req[..pos]).to_ascii_lowercase();
                                let cl = head
                                    .lines()
                                    .find_map(|l| {
                                        l.strip_prefix("content-length:")
                                            .and_then(|v| v.trim().parse::<usize>().ok())
                                    })
                                    .unwrap_or(0);
                                want = Some(pos + 4 + cl);
                            }
                        }
                    }
                }
                let header = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(header.as_bytes()).await;
                let _ = sock.write_all(body).await;
                let _ = sock.flush().await;
            }
        });
        addr
    }

    fn client_for(addr: &str) -> crate::client::ApiClient {
        crate::client::ApiClient::new(&format!("http://{addr}"), Some("tok".to_owned()))
            .expect("client builds")
    }

    fn assert_sa_hint(err: &crate::error::CliError) {
        match err {
            crate::error::CliError::Api(api) => {
                assert_eq!(api.http_status, 403);
                assert_eq!(api.field_reason(), Some("service_account_not_permitted"));
            }
            other => panic!("expected CliError::Api, got {other:?}"),
        }
        assert_eq!(err.suggestion(), Some(crate::error::HINT_SA_NOT_PERMITTED));
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fastio-user-{tag}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[tokio::test]
    async fn upload_entity_asset_keeps_sa_refusal_params() {
        let addr = spawn_one_shot("403 Forbidden", SA_DENIED).await;
        let dir = scratch("up403");
        let file = dir.join("logo.png");
        std::fs::write(&file, b"png").expect("write scratch file");
        let err = super::upload_entity_asset(
            &client_for(&addr),
            "workspace",
            "1234567890123456789",
            "logo",
            file.to_str().expect("utf-8 path"),
        )
        .await
        .expect_err("a 403 must surface as an error");
        let _ = std::fs::remove_dir_all(&dir);
        assert_sa_hint(&err);
    }

    /// The 2xx-with-`result: no` branch keeps the same details.
    #[tokio::test]
    async fn upload_entity_asset_keeps_params_on_2xx_result_no() {
        let addr = spawn_one_shot("200 OK", SA_DENIED).await;
        let dir = scratch("up200");
        let file = dir.join("logo.png");
        std::fs::write(&file, b"png").expect("write scratch file");
        let err = super::upload_entity_asset(
            &client_for(&addr),
            "workspace",
            "1234567890123456789",
            "logo",
            file.to_str().expect("utf-8 path"),
        )
        .await
        .expect_err("result: no must surface as an error");
        let _ = std::fs::remove_dir_all(&dir);
        match err {
            crate::error::CliError::Api(api) => {
                assert_eq!(api.http_status, 200);
                assert_eq!(api.field_reason(), Some("service_account_not_permitted"));
            }
            other => panic!("expected CliError::Api, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_user_asset_keeps_sa_refusal_params() {
        let addr = spawn_one_shot("403 Forbidden", SA_DENIED).await;
        let dir = scratch("read403");
        let out = dir.join("asset.bin");
        let err = super::read_user_asset(&client_for(&addr), "1234567890123456789", "photo", &out)
            .await
            .expect_err("a 403 must surface as an error");
        let _ = std::fs::remove_dir_all(&dir);
        assert_sa_hint(&err);
    }
}

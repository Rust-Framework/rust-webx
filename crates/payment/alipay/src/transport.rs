//! Alipay OpenAPI transport.
//!
//! The gateway is a form POST / redirect URL whose parameters carry an RSA2
//! signature. Responses are JSON objects named after the method
//! (`alipay_trade_query_response` or `error_response`) plus a `sign` sibling;
//! the signature covers the **raw JSON substring** of that response node, so
//! verification works on the exact bytes received and never on a
//! re-serialization of the parsed value.

use rsa::PublicKey;
use serde_json::Value;

use webx_pay::PaymentError;

use crate::mapping::string_field;
use crate::provider::AlipayAccount;
use crate::sign;
use crate::time;
use crate::PROVIDER;

/// Upper bound for a gateway response body.
pub(crate) const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// `alipay.trade.query`.
pub(crate) const METHOD_TRADE_QUERY: &str = "alipay.trade.query";
/// `alipay.trade.close`.
pub(crate) const METHOD_TRADE_CLOSE: &str = "alipay.trade.close";
/// `alipay.trade.refund`.
pub(crate) const METHOD_TRADE_REFUND: &str = "alipay.trade.refund";
/// `alipay.trade.fastpay.refund.query`.
pub(crate) const METHOD_REFUND_QUERY: &str = "alipay.trade.fastpay.refund.query";
/// `alipay.trade.page.pay` (desktop web, redirect).
pub(crate) const METHOD_PAGE_PAY: &str = "alipay.trade.page.pay";
/// `alipay.trade.wap.pay` (mobile web, redirect).
pub(crate) const METHOD_WAP_PAY: &str = "alipay.trade.wap.pay";
/// `alipay.trade.app.pay` (app SDK order string).
pub(crate) const METHOD_APP_PAY: &str = "alipay.trade.app.pay";
/// `alipay.trade.precreate` (QR code).
pub(crate) const METHOD_PRECREATE: &str = "alipay.trade.precreate";

/// Build the signed parameter list for one gateway call.
///
/// Parameters are ASCII-sorted and include the `sign` value; the signature
/// content is the sorted `k=v` list excluding `sign` only (so `sign_type`
/// participates, matching the official SDK).
pub(crate) fn signed_params(
    account: &AlipayAccount,
    method: &str,
    biz_content: &Value,
    notify_url: Option<&str>,
    return_url: Option<&str>,
    now: i64,
) -> Result<Vec<(String, String)>, PaymentError> {
    let mut params = vec![
        ("app_id".to_string(), account.app_id.clone()),
        ("method".to_string(), method.to_string()),
        ("format".to_string(), "JSON".to_string()),
        ("charset".to_string(), "utf-8".to_string()),
        ("sign_type".to_string(), "RSA2".to_string()),
        ("timestamp".to_string(), time::format_datetime(now)),
        ("version".to_string(), "1.0".to_string()),
        (
            "biz_content".to_string(),
            serde_json::to_string(biz_content).map_err(|error| {
                PaymentError::Serialization(format!("cannot encode alipay biz_content: {error}"))
            })?,
        ),
    ];
    if let Some(notify_url) = notify_url.filter(|value| !value.is_empty()) {
        params.push(("notify_url".to_string(), notify_url.to_string()));
    }
    if let Some(return_url) = return_url.filter(|value| !value.is_empty()) {
        params.push(("return_url".to_string(), return_url.to_string()));
    }
    // Certificate mode: the gateway identifies the signing certificate by SN
    // and validates the chain through the Alipay root certificates.
    if let Some(app_cert_sn) = &account.app_cert_sn {
        params.push(("app_cert_sn".to_string(), app_cert_sn.clone()));
    }
    if let Some(root_cert_sn) = &account.alipay_root_cert_sn {
        params.push(("alipay_root_cert_sn".to_string(), root_cert_sn.clone()));
    }
    let content = sign::request_sign_content(&params);
    let signature =
        sign::rsa2_sign(&content, &account.private_key).map_err(PaymentError::internal)?;
    params.push(("sign".to_string(), signature));
    params.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(params)
}

/// `application/x-www-form-urlencoded` encoding of `params`, in the given
/// order (callers pass the sorted list).
pub(crate) fn encode_form(params: &[(String, String)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.extend_pairs(params);
    serializer.finish()
}

/// Signed gateway URL used to redirect a browser (page/WAP pay, GET mode).
pub(crate) fn redirect_url(gateway: &str, params: &[(String, String)]) -> String {
    let gateway = gateway.trim_end_matches('?');
    format!("{gateway}?{}", encode_form(params))
}

/// Verify a gateway response and return `(is_error_response, node)`.
///
/// The signature source is the raw JSON value of the response node, extracted
/// from the received body (see [`extract_sign_source`]). Success responses must
/// be signed; `error_response` payloads are accepted unsigned, matching the
/// official SDK (they are only used for error classification).
pub(crate) fn parse_response(
    raw: &str,
    method: &str,
    public_key: &PublicKey,
) -> Result<(bool, Value), PaymentError> {
    let value: Value = serde_json::from_str(raw).map_err(|error| {
        PaymentError::Serialization(format!("invalid alipay response JSON: {error}"))
    })?;
    let response_node = format!("{}_response", method.replace('.', "_"));
    let is_error = value.get(&response_node).is_none();
    let node_name = if is_error {
        "error_response"
    } else {
        response_node.as_str()
    };
    let node = value.get(node_name).cloned().ok_or_else(|| {
        PaymentError::Serialization(format!(
            "alipay response contains neither `{response_node}` nor `error_response`"
        ))
    })?;
    match value.get("sign").and_then(Value::as_str) {
        Some(signature) => {
            let source = extract_sign_source(raw, node_name)
                .map_err(|reason| PaymentError::signature(PROVIDER, reason))?;
            sign::rsa2_verify(&source, signature, public_key)
                .map_err(|reason| PaymentError::signature(PROVIDER, reason))?;
        }
        None if !is_error => {
            return Err(PaymentError::signature(PROVIDER, "missing signature"));
        }
        None => {}
    }
    Ok((is_error, node))
}

/// Extract the raw JSON value signed by an Alipay gateway response.
///
/// Scans the body for the structurally quoted `"<node_name>"` key, skips the
/// colon/whitespace, then returns the balanced `{...}` (or quoted string for
/// encrypted responses) **exactly as received**. A duplicate response node is
/// rejected: the signature covers the first occurrence while a JSON parser
/// would keep the last one.
pub(crate) fn extract_sign_source(raw: &str, node_name: &str) -> Result<String, &'static str> {
    let quoted = format!("\"{node_name}\"");
    let mut search_from = 0usize;
    while let Some(offset) = raw.get(search_from..).and_then(|rest| rest.find(&quoted)) {
        let key_pos = search_from + offset;
        let structural = matches!(
            raw.get(..key_pos)
                .map(str::trim_end)
                .and_then(|prefix| prefix.chars().next_back()),
            Some('{' | ',')
        );
        if structural {
            let mut cursor = key_pos + quoted.len();
            cursor = skip_whitespace(raw, cursor);
            if !raw.get(cursor..).is_some_and(|rest| rest.starts_with(':')) {
                search_from = key_pos + quoted.len();
                continue;
            }
            cursor = skip_whitespace(raw, cursor + 1);
            let end = if raw.get(cursor..).is_some_and(|rest| rest.starts_with('{')) {
                balanced_object_end(raw, cursor).ok_or("malformed response node")?
            } else if raw.get(cursor..).is_some_and(|rest| rest.starts_with('"')) {
                json_string_end(raw, cursor).ok_or("malformed response string")?
            } else {
                return Err("response node has an unsupported JSON kind");
            };
            if raw.get(end..).is_some_and(|rest| rest.contains(&quoted)) {
                return Err("duplicate response node");
            }
            return raw
                .get(cursor..end)
                .map(str::to_string)
                .ok_or("malformed response node");
        }
        search_from = key_pos + quoted.len();
    }
    Err("signature source not found")
}

fn skip_whitespace(raw: &str, cursor: usize) -> usize {
    match raw.get(cursor..) {
        Some(rest) => cursor + (rest.len() - rest.trim_start().len()),
        None => cursor,
    }
}

/// End index (exclusive) of the balanced JSON object starting at `start`.
fn balanced_object_end(raw: &str, start: usize) -> Option<usize> {
    let bytes = raw.as_bytes();
    let mut depth = 0usize;
    let mut in_quotes = false;
    let mut escapes = 0usize;
    let mut index = start;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'"' && escapes.is_multiple_of(2) {
            in_quotes = !in_quotes;
        } else if byte == b'{' && !in_quotes {
            depth += 1;
        } else if byte == b'}' && !in_quotes {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index + 1);
            }
        }
        if byte == b'\\' {
            escapes += 1;
        } else {
            escapes = 0;
        }
        index += 1;
    }
    None
}

/// End index (exclusive) of the JSON string starting at `start` (a quote).
fn json_string_end(raw: &str, start: usize) -> Option<usize> {
    let bytes = raw.as_bytes();
    let mut escapes = 0usize;
    let mut index = start + 1;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'"' && escapes.is_multiple_of(2) {
            return Some(index + 1);
        }
        if byte == b'\\' {
            escapes += 1;
        } else {
            escapes = 0;
        }
        index += 1;
    }
    None
}

/// Turn a verified gateway response node into a success payload or a typed
/// [`PaymentError`].
pub(crate) fn ensure_success(
    is_error: bool,
    node: Value,
    request_id: Option<String>,
) -> Result<Value, PaymentError> {
    if is_error {
        return Err(map_gateway_error(&node, request_id));
    }
    match string_field(&node, "code") {
        Some(code) if code != "10000" => Err(map_gateway_error(&node, request_id)),
        _ => Ok(node),
    }
}

/// Map a gateway error node (`error_response` or a non-`10000` response code).
pub(crate) fn map_gateway_error(node: &Value, request_id: Option<String>) -> PaymentError {
    let raw_code = string_field(node, "code").unwrap_or_default();
    let sub_code = string_field(node, "sub_code").unwrap_or_default();
    let message = string_field(node, "sub_msg")
        .filter(|value| !value.is_empty())
        .or_else(|| string_field(node, "msg").filter(|value| !value.is_empty()))
        .unwrap_or_else(|| "alipay request failed".to_string());
    if sub_code == "ACQ.TRADE_NOT_EXIST" {
        return PaymentError::not_found(PROVIDER, message);
    }
    const CONFLICT_CODES: &[&str] = &[
        "ACQ.TRADE_STATUS_ERROR",
        "ACQ.TRADE_HAS_CLOSE",
        "ACQ.TRADE_HAS_FINISHED",
        "ACQ.REFUND_AMT_NOT_EQUAL_TOTAL",
        "ACQ.TRADE_NOT_ALLOW_REFUND",
    ];
    if CONFLICT_CODES.iter().any(|code| sub_code.starts_with(code)) {
        return PaymentError::conflict(PROVIDER, message);
    }
    let retryable = matches!(raw_code.as_str(), "20000" | "20001" | "50000")
        || sub_code.ends_with("SYSTEM_ERROR");
    let reported_code = if sub_code.is_empty() {
        raw_code
    } else {
        sub_code.clone()
    };
    let http_status = if reported_code.starts_with('4') {
        Some(400)
    } else if retryable {
        Some(500)
    } else {
        None
    };
    PaymentError::provider(
        PROVIDER,
        (!reported_code.is_empty()).then_some(reported_code),
        message,
        http_status,
        request_id,
        retryable,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    fn verify_signed_params(params: &[(String, String)]) {
        assert!(
            params.windows(2).all(|pair| pair[0].0 <= pair[1].0),
            "params sorted"
        );
        let signature = params
            .iter()
            .find(|(key, _)| key == "sign")
            .map(|(_, value)| value.clone())
            .expect("sign parameter");
        let content = sign::request_sign_content(params);
        assert!(
            sign::rsa2_verify(&content, &signature, test_support::merchant_public_key()).is_ok()
        );
        assert!(content.contains("sign_type=RSA2"));
    }

    #[test]
    fn signed_parameters_verify_and_are_sorted() {
        let account = test_support::account();
        let params = signed_params(
            account,
            METHOD_TRADE_QUERY,
            &serde_json::json!({"out_trade_no": "ORDER-1"}),
            Some("https://example.com/notify"),
            None,
            1_700_000_000,
        )
        .expect("signed params");
        verify_signed_params(&params);
        let fields: Vec<(&str, &str)> = params
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        assert_eq!(fields[0].0, "app_id");
        assert!(fields.contains(&("biz_content", r#"{"out_trade_no":"ORDER-1"}"#)));
        assert!(fields.contains(&("method", METHOD_TRADE_QUERY)));
        assert!(fields.contains(&("notify_url", "https://example.com/notify")));
        assert!(fields.contains(&("timestamp", "2023-11-15 06:13:20")));
        assert!(fields.contains(&("version", "1.0")));
        assert!(fields.contains(&("format", "JSON")));
        assert!(fields.contains(&("charset", "utf-8")));
    }

    #[test]
    fn certificate_mode_adds_certificate_sns() {
        let account = test_support::certificate_account_options()
            .into_account(false)
            .expect("certificate account");
        let params = signed_params(
            &account,
            METHOD_TRADE_REFUND,
            &serde_json::json!({"out_trade_no": "ORDER-1"}),
            None,
            None,
            1_700_000_000,
        )
        .expect("signed params");
        verify_signed_params(&params);
        let fields: Vec<&str> = params.iter().map(|(key, _)| key.as_str()).collect();
        assert!(fields.contains(&"app_cert_sn"));
        assert!(fields.contains(&"alipay_root_cert_sn"));
    }

    #[test]
    fn form_encoding_and_redirect_url() {
        let params = vec![
            ("a".to_string(), "A B/C".to_string()),
            ("biz_content".to_string(), r#"{"k":"v"}"#.to_string()),
        ];
        assert_eq!(
            encode_form(&params),
            "a=A+B%2FC&biz_content=%7B%22k%22%3A%22v%22%7D"
        );
        assert_eq!(
            redirect_url("https://openapi.alipay.com/gateway.do", &params),
            "https://openapi.alipay.com/gateway.do?a=A+B%2FC&biz_content=%7B%22k%22%3A%22v%22%7D"
        );
    }

    #[test]
    fn extracts_raw_substring_without_re_serializing() {
        let node = r#"{ "trade_status": "TRADE_SUCCESS", "buyer_logon_id": "a\/b", "note": "}{\"quoted\"}", "nested": {"x": 1} }"#;
        let raw = format!("{{\"alipay_trade_query_response\":{node},\"sign\":\"ignored\"}}");
        assert_eq!(
            extract_sign_source(&raw, "alipay_trade_query_response").unwrap(),
            node
        );
    }

    #[test]
    fn extracts_encrypted_string_responses() {
        let raw = r#"{"alipay_trade_query_response":"ZW5jcnlwdGVk==","sign":"S"}"#;
        assert_eq!(
            extract_sign_source(raw, "alipay_trade_query_response").unwrap(),
            "\"ZW5jcnlwdGVk==\""
        );
    }

    #[test]
    fn rejects_duplicate_and_missing_nodes() {
        let raw = r#"{"alipay_trade_query_response":{"a":1},"x":0,"alipay_trade_query_response":{"a":2},"sign":"S"}"#;
        assert_eq!(
            extract_sign_source(raw, "alipay_trade_query_response"),
            Err("duplicate response node")
        );
        assert_eq!(
            extract_sign_source(r#"{"sign":"S"}"#, "alipay_trade_query_response"),
            Err("signature source not found")
        );
        // A quoted value that merely equals the node name is not the node.
        assert_eq!(
            extract_sign_source(
                r#"{"note":"alipay_trade_query_response","sign":"S"}"#,
                "alipay_trade_query_response"
            ),
            Err("signature source not found")
        );
    }

    #[test]
    fn parses_and_verifies_a_signed_success_response() {
        let node = r#"{"code":"10000","msg":"Success","trade_no":"2023111522001","out_trade_no":"ORDER-1","trade_status":"TRADE_SUCCESS","total_amount":"99.00","receipt_amount":"99.00"}"#;
        let raw = test_support::signed_gateway_body(METHOD_TRADE_QUERY, node);
        let (is_error, parsed) =
            parse_response(&raw, METHOD_TRADE_QUERY, test_support::alipay_public_key()).unwrap();
        assert!(!is_error);
        let node = ensure_success(is_error, parsed, None).unwrap();
        assert_eq!(
            string_field(&node, "trade_status").as_deref(),
            Some("TRADE_SUCCESS")
        );
    }

    #[test]
    fn rejects_tampered_response_content() {
        let node = r#"{"code":"10000","trade_status":"TRADE_SUCCESS"}"#;
        let raw = test_support::signed_gateway_body(METHOD_TRADE_QUERY, node);
        let tampered = raw.replacen("\"code\":\"10000\"", "\"code\":\"10001\"", 1);
        let error = parse_response(
            &tampered,
            METHOD_TRADE_QUERY,
            test_support::alipay_public_key(),
        )
        .unwrap_err();
        assert_eq!(error.http_status(), 400);
        assert_eq!(error.public_detail(), Some("signature mismatch"));
    }

    #[test]
    fn success_response_requires_a_signature() {
        let raw = r#"{"alipay_trade_query_response":{"code":"10000"}}"#;
        let error =
            parse_response(raw, METHOD_TRADE_QUERY, test_support::alipay_public_key()).unwrap_err();
        assert_eq!(error.public_detail(), Some("missing signature"));
    }

    #[test]
    fn unsigned_error_response_is_mapped() {
        let raw = r#"{"error_response":{"code":"40004","msg":"Business Failed","sub_code":"ACQ.TRADE_NOT_EXIST","sub_msg":"交易不存在"}}"#;
        let (is_error, node) =
            parse_response(raw, METHOD_TRADE_QUERY, test_support::alipay_public_key()).unwrap();
        assert!(is_error);
        let error = ensure_success(is_error, node, None).unwrap_err();
        assert_eq!(error.http_status(), 404);
    }

    #[test]
    fn maps_business_errors_inside_a_success_envelope() {
        let node = serde_json::json!({
            "code": "40004",
            "msg": "Business Failed",
            "sub_code": "ACQ.TRADE_STATUS_ERROR",
            "sub_msg": "交易状态不合法"
        });
        let error = ensure_success(false, node, None).unwrap_err();
        assert_eq!(error.http_status(), 409);

        let node = serde_json::json!({
            "code": "20000",
            "msg": "Service Currently Unavailable",
            "sub_code": "aop.unknow-error"
        });
        let error = ensure_success(true, node, Some("trace-1".to_string())).unwrap_err();
        assert!(error.retryable());
        assert_eq!(error.http_status(), 502);

        let node = serde_json::json!({"code": "40002", "msg": "Invalid Arguments"});
        let error = ensure_success(false, node, None).unwrap_err();
        assert_eq!(error.http_status(), 400);
    }

    #[test]
    fn signed_error_response_is_verified_too() {
        let raw = test_support::signed_error_body(
            r#"{"code":"40004","msg":"Business Failed","sub_code":"ACQ.TRADE_NOT_EXIST"}"#,
        );
        let (is_error, node) =
            parse_response(&raw, METHOD_TRADE_QUERY, test_support::alipay_public_key()).unwrap();
        assert!(is_error);
        assert!(ensure_success(is_error, node, None).is_err());

        let tampered = raw.replace("ACQ.TRADE_NOT_EXIST", "ACQ.TRADE_EXISTS");
        assert!(parse_response(
            &tampered,
            METHOD_TRADE_QUERY,
            test_support::alipay_public_key()
        )
        .is_err());
    }
}

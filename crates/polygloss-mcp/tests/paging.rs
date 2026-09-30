//! Cursors, page fitting and error mapping (T4.4).

use polygloss_core::review::CoreError;
use polygloss_mcp::paging::{base64url_decode, base64url_encode};
use polygloss_mcp::{
    ApiError, ApiErrorCode, Cursor, PAGE_MAX_CHARS, decode_cursor, encode_cursor, fit_page,
    tool_result,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct After {
    updated_at: i64,
    review_id: String,
}

fn sample_cursor() -> Cursor {
    Cursor::new(
        "reviews",
        "repo=/tmp/x;status=open",
        &After {
            updated_at: 1_790_000_000_000,
            review_id: "0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".into(),
        },
    )
    .unwrap()
}

#[test]
fn cursor_roundtrip_and_tamper_rejected() {
    let c = sample_cursor();
    let s = encode_cursor(&c);
    // Opaque and URL-safe: base64url without padding.
    assert!(
        s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "{s}"
    );
    let back = decode_cursor(&s).unwrap();
    assert_eq!(back, c);
    assert_eq!(
        back.after_as::<After>().unwrap(),
        c.after_as::<After>().unwrap()
    );
    back.check_query("reviews", "repo=/tmp/x;status=open")
        .unwrap();
    assert_eq!(
        back.check_query("threads", "repo=/tmp/x;status=open")
            .unwrap_err()
            .code,
        ApiErrorCode::Conflict
    );
    assert_eq!(
        back.check_query("reviews", "repo=/tmp/y").unwrap_err().code,
        ApiErrorCode::Conflict
    );

    // Every single-character edit is rejected, never decoded to another cursor.
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    for i in 0..s.len() {
        let mut bytes = s.clone().into_bytes();
        let orig = bytes[i];
        let replacement = alphabet
            .iter()
            .copied()
            .find(|&b| b != orig)
            .expect("alphabet has other characters");
        bytes[i] = replacement;
        let edited = String::from_utf8(bytes).unwrap();
        let err = decode_cursor(&edited).expect_err(&format!("edit at {i} accepted"));
        assert_eq!(err.code, ApiErrorCode::Conflict, "{err}");
    }

    // An edited payload re-encoded without the check is rejected too.
    let json = String::from_utf8(base64url_decode(&s).unwrap()).unwrap();
    let forged = json.replace("1790000000000", "1790000000001");
    assert_ne!(forged, json);
    assert_eq!(
        decode_cursor(&base64url_encode(forged.as_bytes()))
            .unwrap_err()
            .code,
        ApiErrorCode::Conflict
    );

    // Truncated, padded, empty and foreign strings.
    for bad in [
        &s[..s.len() - 1],
        &format!("{s}="),
        "",
        "not a cursor",
        "e30",
    ] {
        assert_eq!(decode_cursor(bad).unwrap_err().code, ApiErrorCode::Conflict, "{bad:?}");
    }
}

#[test]
fn base64url_roundtrips_every_length() {
    for len in 0..64 {
        let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
        let s = base64url_encode(&data);
        assert!(!s.contains('='));
        assert_eq!(base64url_decode(&s).unwrap(), data, "len {len}");
    }
    assert_eq!(base64url_encode(b"foob"), "Zm9vYg");
    assert_eq!(base64url_decode("Zm9vYmFy").unwrap(), b"foobar");
    // Non-zero spare bits after the last byte are not canonical.
    assert!(base64url_decode("Zm9vYh").is_none());
}

#[derive(Serialize, Clone)]
struct Item {
    id: usize,
    excerpt: String,
}

fn items(n: usize, excerpt_len: usize) -> Vec<Item> {
    (0..n)
        .map(|id| Item {
            id,
            excerpt: "é".repeat(excerpt_len),
        })
        .collect()
}

#[test]
fn fit_page_stays_under_60k_chars() {
    assert_eq!(PAGE_MAX_CHARS, 60_000);

    // 200 items of ~1k chars each: far over the budget.
    let all = items(200, 1_000);
    let (page, truncated) = fit_page(all.clone(), PAGE_MAX_CHARS);
    assert!(truncated);
    let text = serde_json::to_string(&page).unwrap();
    assert!(text.chars().count() <= PAGE_MAX_CHARS, "{}", text.len());
    // Greedy: one more item would not have fit.
    let mut one_more = page.clone();
    one_more.push(all[page.len()].clone());
    assert!(serde_json::to_string(&one_more).unwrap().chars().count() > PAGE_MAX_CHARS);
    // Prefix order is kept.
    assert!(page.iter().enumerate().all(|(i, it)| it.id == i));

    // A small list fits whole.
    let (page, truncated) = fit_page(items(10, 100), PAGE_MAX_CHARS);
    assert_eq!(page.len(), 10);
    assert!(!truncated);

    // Empty.
    let (page, truncated) = fit_page(Vec::<Item>::new(), PAGE_MAX_CHARS);
    assert!(page.is_empty() && !truncated);

    // A single item larger than the budget is still returned (progress), and the
    // rest is reported as truncated.
    let (page, truncated) = fit_page(items(3, 100), 50);
    assert_eq!(page.len(), 1);
    assert!(truncated);
}

fn is_error(r: &rmcp::model::CallToolResult) -> bool {
    r.is_error == Some(true)
}

fn text_json(r: &rmcp::model::CallToolResult) -> Value {
    let text = &r.content[0].as_text().expect("text content").text;
    serde_json::from_str(text).unwrap()
}

#[test]
fn error_codes_map_to_is_error_results() {
    // Every code serializes to its §15.1 name and round-trips.
    let names = [
        "not_found",
        "repo_not_found",
        "objects_missing",
        "invalid_anchor",
        "cap_exceeded",
        "forbidden",
        "app_unavailable",
        "conflict",
        "internal",
    ];
    for (code, name) in ApiErrorCode::ALL.iter().zip(names) {
        assert_eq!(code.as_str(), name);
        assert_eq!(serde_json::to_value(code).unwrap(), json!(name));
        assert_eq!(ApiErrorCode::parse(name), Some(*code));

        let r = tool_result::<Value>(Err(ApiError::new(*code, format!("boom {name}"))));
        assert!(is_error(&r));
        let expected = json!({ "code": name, "message": format!("boom {name}") });
        assert_eq!(r.structured_content, Some(expected.clone()));
        assert_eq!(text_json(&r), expected);
    }

    // Success: structuredContent plus the same JSON as text.
    let ok = tool_result::<Value>(Ok(json!({ "review_id": "r1", "n": 2 })));
    assert!(!is_error(&ok));
    assert_eq!(
        ok.structured_content,
        Some(json!({ "review_id": "r1", "n": 2 }))
    );
    assert_eq!(text_json(&ok), json!({ "review_id": "r1", "n": 2 }));

    // Core errors map through `CoreError::code`.
    let cases: Vec<(CoreError, ApiErrorCode)> = vec![
        (
            CoreError::NotFound {
                what: "thread",
                id: "t1".into(),
            },
            ApiErrorCode::NotFound,
        ),
        (
            CoreError::Ambiguous {
                prefix: "abcd1234".into(),
                matches: vec!["abcd1234aa".into(), "abcd1234bb".into()],
            },
            ApiErrorCode::NotFound,
        ),
        (CoreError::RepoNotFound("d".into()), ApiErrorCode::RepoNotFound),
        (CoreError::InvalidAnchor("line 9".into()), ApiErrorCode::InvalidAnchor),
        (CoreError::CapExceeded { cap: 50 }, ApiErrorCode::CapExceeded),
        (CoreError::Forbidden("not yours".into()), ApiErrorCode::Forbidden),
        (CoreError::Conflict("x".into()), ApiErrorCode::Conflict),
        (CoreError::InvalidRequest("empty body".into()), ApiErrorCode::Conflict),
    ];
    for (core, code) in cases {
        let msg = core.to_string();
        let api = ApiError::from(core);
        assert_eq!(api.code, code);
        assert_eq!(api.message, msg);
        let r = tool_result::<Value>(Err(api));
        assert!(is_error(&r));
        assert_eq!(text_json(&r)["code"], json!(code.as_str()));
    }
    // The ambiguous-prefix message lists the matches (§15.1).
    let amb = ApiError::from(CoreError::Ambiguous {
        prefix: "abcd1234".into(),
        matches: vec!["abcd1234aa".into(), "abcd1234bb".into()],
    });
    assert!(amb.message.contains("abcd1234aa") && amb.message.contains("abcd1234bb"));
}

//! 云歌单双加密通道：请求体 AES 加密，会话密钥 RSA 加密。

use lyra_proto::consts::{APPID, CLIENTVER, RSA_PUBLIC, SALT_MD5};
use lyra_proto::{md5_hex, random_aes_secret, rsa_pkcs1_encrypt_hex, secret_decrypt_bytes, secret_encrypt_bytes};
use serde_json::Value;

use crate::codec;
use crate::error::{Error, Result};
use crate::spec::{Session, now_secs};
use crate::transport::{Method, Request};

/// 云歌单服务路由头。
pub const CLOUD_ROUTER: &str = "cloudlist.service.kugou.com";

/// 构造云歌单双加密请求，返回请求对象与 AES 会话密钥。
pub fn build(session: &Session, path: &str, body: &Value) -> Result<(Request, String)> {
    let aes = random_aes_secret();
    let clienttime = now_secs();

    let key = md5_hex(format!("{APPID}{SALT_MD5}{CLIENTVER}{clienttime}").as_bytes()).to_lowercase();
    let portrait = serde_json::json!({
        "aes": aes,
        "uid": session.userid(),
        "token": session.token(),
    })
    .to_string();
    let p = rsa_pkcs1_encrypt_hex(portrait.as_bytes(), RSA_PUBLIC)?.to_uppercase();

    let body_bytes = secret_encrypt_bytes(&body.to_string(), &aes)?;

    let mut request = Request::new(Method::Post, crate::spec::combine_urls(crate::spec::GATEWAY, path));
    request.query = vec![
        ("appid".to_owned(), APPID.to_string()),
        ("clientver".to_owned(), CLIENTVER.to_string()),
        ("mid".to_owned(), session.device.mid.clone()),
        ("clienttime".to_owned(), clienttime.to_string()),
        ("key".to_owned(), key),
        ("dfid".to_owned(), session.dfid().to_owned()),
        ("p".to_owned(), p),
    ];
    request.headers = crate::spec::merge_headers(vec![
        ("Content-Type".to_owned(), "application/json;charset=utf-8".to_owned()),
        ("x-router".to_owned(), CLOUD_ROUTER.to_owned()),
        ("dfid".to_owned(), session.dfid().to_owned()),
        ("clienttime".to_owned(), clienttime.to_string()),
        ("mid".to_owned(), session.device.mid.clone()),
        ("kg-rc".to_owned(), lyra_proto::consts::KG_RC.to_owned()),
        ("kg-thash".to_owned(), lyra_proto::consts::KG_THASH.to_owned()),
        ("kg-rec".to_owned(), lyra_proto::consts::KG_REC.to_owned()),
        ("kg-rf".to_owned(), lyra_proto::consts::KG_RF.to_owned()),
    ]);
    request.body = Some(body_bytes);
    Ok((request, aes))
}

/// 解密并校验云歌单响应内容。
pub fn parse(cipher: &[u8], aes: &str) -> Result<Value> {
    let decrypted = secret_decrypt_bytes(cipher, aes).ok().filter(|bytes| !bytes.is_empty());
    let text = match decrypted {
        Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        None => String::from_utf8_lossy(cipher).into_owned(),
    };
    let value: Value =
        serde_json::from_str(&text).map_err(|_| Error::Decode(format!("cloudlist 响应不是合法 JSON: {text}")))?;
    codec::check_business(&value)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use lyra_proto::secret_encrypt_with;

    fn session() -> Session {
        Session::new(lyra_proto::Device {
            guid: "e8456cee7fd2348cfa239342b10a23bd".to_owned(),
            mid: "12345678901234567890".to_owned(),
            mac: "02:00:00:00:00:00".to_owned(),
            dev: "ABCDEFGHIJ".to_owned(),
            webgl: "1".to_owned(),
            dfid: "DfidValue".to_owned(),
        })
        .with_account(crate::spec::Account {
            userid: 2_043_841_403,
            token: "TOKEN123".to_owned(),
            vip_type: 0,
            vip_token: String::new(),
        })
    }

    #[test]
    fn builds_the_documented_query_and_headers() {
        let (request, aes) = build(&session(), "/v1/modify_song_sort", &serde_json::json!({"listid": 2})).unwrap();
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.get_header("x-router"), Some(CLOUD_ROUTER));
        assert_eq!(
            request.get_header("Content-Type"),
            Some("application/json;charset=utf-8")
        );
        let names: Vec<&str> = request.query.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            names,
            vec!["appid", "clientver", "mid", "clienttime", "key", "dfid", "p"]
        );
        // 校验请求签名格式。
        let clienttime = request
            .query
            .iter()
            .find(|(k, _)| k == "clienttime")
            .map(|(_, v)| v.clone())
            .unwrap();
        let expected = md5_hex(format!("{APPID}{SALT_MD5}{CLIENTVER}{clienttime}").as_bytes());
        assert_eq!(request.query.iter().find(|(k, _)| k == "key").unwrap().1, expected);
        // 校验 RSA 加密参数为大写十六进制。
        let p = &request.query.iter().find(|(k, _)| k == "p").unwrap().1;
        assert_eq!(p.len(), 256);
        assert!(
            p.chars().all(|c| !c.is_ascii_lowercase() || c.is_ascii_digit()),
            "p 必须全大写"
        );
        assert_eq!(aes.len(), 6);
    }

    #[test]
    fn body_is_aes_encrypted_with_the_session_secret() {
        let (request, aes) = build(&session(), "/v1/x", &serde_json::json!({"a": 1})).unwrap();
        let plain =
            String::from_utf8(lyra_proto::secret_decrypt_bytes(&request.body.clone().unwrap(), &aes).unwrap()).unwrap();
        assert_eq!(plain, r#"{"a":1}"#);
    }

    #[test]
    fn parses_encrypted_response() {
        let aes = "Ab3xZ9";
        let cipher = secret_encrypt_with(r#"{"status":1,"data":{"ok":true}}"#, aes).unwrap();
        let raw = lyra_proto::base64_decode(&cipher.base64).unwrap();
        let value = parse(&raw, aes).unwrap();
        assert_eq!(value["data"]["ok"], true);
    }

    #[test]
    fn falls_back_to_plaintext_when_server_does_not_encrypt() {
        let value = parse(br#"{"status":1,"data":[]}"#, "Ab3xZ9").unwrap();
        assert_eq!(value["status"], 1);
    }

    #[test]
    fn rejects_business_failure() {
        let aes = "Ab3xZ9";
        let cipher = secret_encrypt_with(r#"{"status":0,"msg":"nope"}"#, aes).unwrap();
        let raw = lyra_proto::base64_decode(&cipher.base64).unwrap();
        assert!(parse(&raw, aes).is_err());
    }
}

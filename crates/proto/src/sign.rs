//! 请求签名算法（Android、Web、设备注册、signKey 与 signParamsKey）。

use md5::{Digest as _, Md5};

use crate::consts::{SALT_MD5, SALT_REGISTER, SALT_SIGNKEY, SALT_WEB};
use crate::params::Params;

fn md5_concat(parts: &[&[u8]]) -> String {
    let mut h = Md5::new();
    for p in parts {
        h.update(p);
    }
    hex::encode(h.finalize())
}

/// Android 端请求签名：md5(SALT_MD5 + sorted_kv + body + SALT_MD5)。
#[must_use]
pub fn signature_android(params: &Params, body: &str) -> String {
    md5_concat(&[
        SALT_MD5.as_bytes(),
        params.sorted_kv().as_bytes(),
        body.as_bytes(),
        SALT_MD5.as_bytes(),
    ])
}

/// Web 端请求签名：md5(SALT_WEB + sorted_kv + body + SALT_WEB)。
#[must_use]
pub fn signature_web(params: &Params, body: &str) -> String {
    md5_concat(&[
        SALT_WEB.as_bytes(),
        params.sorted_kv().as_bytes(),
        body.as_bytes(),
        SALT_WEB.as_bytes(),
    ])
}

/// 设备注册请求签名：仅按参数值排序后拼接并加盐计算 MD5。
#[must_use]
pub fn signature_register(params: &Params) -> String {
    md5_concat(&[
        SALT_REGISTER.as_bytes(),
        params.sorted_values().as_bytes(),
        SALT_REGISTER.as_bytes(),
    ])
}

/// 歌曲播放地址授权密钥 sign_key：md5(hash + SALT_SIGNKEY + appid + mid + userid)。
#[must_use]
pub fn sign_key(hash: &str, mid: &str, userid: i64, appid: u32) -> String {
    md5_concat(&[
        hash.as_bytes(),
        SALT_SIGNKEY.as_bytes(),
        appid.to_string().as_bytes(),
        mid.as_bytes(),
        userid.to_string().as_bytes(),
    ])
}

/// 电台接口参数密钥 sign_params_key：md5(appid + SALT_MD5 + clientver + data)。
#[must_use]
pub fn sign_params_key(data: i64, appid: u32, clientver: u32) -> String {
    md5_concat(&[
        appid.to_string().as_bytes(),
        SALT_MD5.as_bytes(),
        clientver.to_string().as_bytes(),
        data.to_string().as_bytes(),
    ])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::consts::{APPID, CLIENTVER};

    #[test]
    fn sign_key_and_params_key_are_stable() {
        // 验证固定测试向量的签名计算稳定性
        assert_eq!(
            sign_key("HASHABCDEF0123456789", "12345678901234567890", 2043841403, APPID),
            "b17dc2425bb03196dbff0a4e502fc54c"
        );
        assert_eq!(
            sign_params_key(1_750_000_000_000, APPID, CLIENTVER),
            "c86cabec1fccc685f21b02d5e2400407"
        );
    }

    #[test]
    fn signature_register_sorts_values_only() {
        let mut p = Params::new();
        p.set("dfid", "ABC")
            .set("mid", "99")
            .set("uuid", "-")
            .set("platid", 1)
            .set("part", 1);
        assert_eq!(signature_register(&p), "230e3285ab006712c1e80921c6d92d4b");
    }
}

//! 酷狗概念版协议原语：纯函数、零 IO，包含签名、密码学、设备身份与行为指纹。
#![forbid(unsafe_code)]

pub mod cipher;
pub mod consts;
pub mod error;
pub mod fingerprint;
pub mod identity;
pub mod params;
pub mod rng;
pub mod sign;

pub use cipher::{
    AesBox, SecretBox, aes_decrypt_derived, aes_encrypt_derived, aes_encrypt_derived_with, aes_encrypt_with_key_iv,
    base64_decode, base64_encode, fingerprint_edt, krc_decode, krc_decode_base64, md5_hex, random_aes_secret,
    random_string, rsa_oaep_sha256_base64, rsa_pkcs1_encrypt_hex, rsa_raw_encrypt_hex, secret_decrypt_base64,
    secret_decrypt_bytes, secret_encrypt, secret_encrypt_bytes, secret_encrypt_with, sha256_hex,
};
pub use error::{Error, Result};
pub use fingerprint::{Draw, SimInputs, Simulate, edt_events, simulate, simulate_now};
pub use identity::{Device, mid_from_guid};
pub use params::{Params, Val};
pub use rng::{JsRng, Mulberry32, OsRng};
pub use sign::{sign_key, sign_params_key, signature_android, signature_register, signature_web};

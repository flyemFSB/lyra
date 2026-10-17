//! 黄金向量测试（对齐逆向参考实现）。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use lyra_proto::consts::{APPID, CLIENTVER};
use lyra_proto::fingerprint::{SimInputs, edt_events, simulate};
use lyra_proto::params::Params;
use lyra_proto::rng::Mulberry32;
use lyra_proto::{cipher, sign};

const FP_SEED: u32 = 0x00C0_FFEE;
const FP_TS: i64 = 1_750_000_000_000;
const FP_MID: &str = "12345678901234567890";
const FP_USERID: &str = "2043841403";
const FP_DFID: &str = "abcdef1234";
const FP_WEBGL: &str = "9876543210987654321";

const GOLDEN_EVENTS: &str = include_str!("golden/fingerprint_events.txt");
const GOLDEN_EDT: &str = include_str!("golden/fingerprint_edt.txt");
const GOLDEN_SID: &str = include_str!("golden/fingerprint_sid.txt");

fn p1() -> Params {
    let mut p = Params::new();
    p.set("dfid", "-")
        .set("mid", "12345678901234567890")
        .set("uuid", "-")
        .set("appid", APPID)
        .set("clientver", CLIENTVER)
        .set("clienttime", 1_750_000_000_i64)
        .set("token", "TOKEN123")
        .set("userid", 2_043_841_403_i64);
    p
}

// 签名测试

#[test]
fn signature_android_with_body() {
    let body = r#"{"listid":2,"userid":2043841403}"#;
    assert_eq!(sign::signature_android(&p1(), body), "af7961385bebe3d3f3208eec03683788");
}

#[test]
fn signature_android_with_empty_body() {
    assert_eq!(sign::signature_android(&p1(), ""), "1644d1568d43ab9103243e79d127793e");
}

#[test]
fn signature_web_with_body() {
    let body = r#"{"a":1}"#;
    assert_eq!(sign::signature_web(&p1(), body), "c12b4c8dce7e25a2e9da6e51850eadff");
}

#[test]
fn signature_android_with_astral_keyword() {
    // 包含中文与 Emoji 时的排序与拼接验证
    let mut p = Params::new();
    p.set("keyword", "周杰伦 🎵")
        .set("page", 1)
        .set("pagesize", 30)
        .set("clientver", 201)
        .set("area_code", 1)
        .set("dopicfull", 1)
        .set("platform", "AndroidFilter")
        .set("iscorrection", 1)
        .set("privilegefilter", 0);
    assert_eq!(sign::signature_android(&p, ""), "e642a9527380364fd6c8b2080bc74a6f");
}

#[test]
fn signature_android_renders_object_values_as_json() {
    let mut p = Params::new();
    p.set("a", 1)
        .set("obj", serde_json::json!({"z": 1, "a": [1, 2, {"q": "x"}]}))
        .set("s", "v");
    assert_eq!(sign::signature_android(&p, ""), "54c2b61dd1e9550ed43e3ba1dc63c020");
}

// 密码学测试

#[test]
fn aes_derived_matches_reference() {
    // 派生密钥 AES 加密一致性
    let plain = r#"{"mobile":"13800138000","code":"123456"}"#;
    let hex = cipher::aes_encrypt_with_key_iv_derived(plain, "abcdefghijklmnop").unwrap();
    assert_eq!(
        hex,
        "c6dac6892d9b32a77fc2f9ada5c749b10f7fd9c0bb00920206732a534aaca71c6271382834f86dffa884cd12b1542c24"
    );
    assert_eq!(
        cipher::aes_decrypt_derived(&hex, "abcdefghijklmnop").unwrap(),
        plain.as_bytes()
    );
}

#[test]
fn aes_derived_handles_non_ascii() {
    let hex = cipher::aes_encrypt_with_key_iv_derived("中文 payload 🎵", "zzzz").unwrap();
    assert_eq!(hex, "d77e7206d069247370cba753493eb478a91789e8a6deef96f8d7436617a8d563");
}

#[test]
fn login_t1_matches_reference() {
    let t1 = cipher::aes_encrypt_with_key_iv("|1750000000000", lyra_proto::consts::T1_KEY, lyra_proto::consts::T1_IV)
        .unwrap();
    assert_eq!(t1, "6072c7d7118cf1b5116f7d263a120e87");
}

#[test]
fn login_t2_matches_reference() {
    let plain =
        "e1f2a3b4c5d6e7f8091a2b3c4d5e6f70|0f607264fc6318a92b9e13c65db7cd3c|02:00:00:00:00:00|ABCDEFGHIJ|1750000000000";
    let t2 = cipher::aes_encrypt_with_key_iv(plain, lyra_proto::consts::T2_KEY, lyra_proto::consts::T2_IV).unwrap();
    assert_eq!(
        t2,
        "66bfd0912b1883fbaa4a811aa10517de06fa51f915fb582e05a4da9cd933797d9808dcc361cd6bbe1d172e9e3b5917cd1dd3ae8af1618a5cff545b5bb5e2e688950d85bcaa412a83f893403591e16f7463de97d1b576703d37cf95ebdba8517beb70d45bdf9eff0dca227b67a0485c72"
    );
}

#[test]
fn cloudlist_body_matches_reference() {
    let plain = r#"{"aes":"Ab3xZ9","uid":2043841403,"token":"T"}"#;
    let encrypted = cipher::secret_encrypt_with(plain, "123456").unwrap();
    assert_eq!(
        encrypted.base64,
        "R+ulN9ILbTY+9e9cnaPldxJzpYTo4nkuhxaRN8sEjPa4nefAkikM3FiHmTuSVIR/"
    );
    assert_eq!(
        cipher::secret_decrypt_base64(&encrypted.base64, "123456").unwrap(),
        plain.as_bytes()
    );
}

#[test]
fn rsa_raw_matches_reference_byte_for_byte() {
    // 无填充 RSA 模幂加密（用于 song_url_auth 的 pk 字段）
    let hex = cipher::rsa_raw_encrypt_hex(
        br#"{"token":"T","clienttime":1750000000}"#,
        lyra_proto::consts::RSA_PUBLIC,
    )
    .unwrap();
    assert_eq!(
        hex,
        "23725481fda6d57b832b59991358cd3899428aa98730c93d5bbd07f7d181698acef6695e9d8dd37bfe5289f006e42b302386ea2736a9682ac6972ef223fe29c2f303eb896fd8317931132cd0a02c2691b2b9f98f18b535dfb785be8b0fd75fa90167b3112170efa9d17d4a99d609e80e492f300a357421d7d715e463f1ce17e7"
    );
}

#[test]
fn pem_keys_are_pinned_by_hash() {
    // 校验公钥指纹，防误替换
    assert_eq!(
        cipher::sha256_hex(lyra_proto::consts::RSA_PUBLIC.as_bytes()),
        "294df941e0bea5640abbcc24842d3a8d082c7d15169da587432e8cc6eec697fc"
    );
    assert_eq!(
        cipher::sha256_hex(lyra_proto::consts::FP_RSA_PUBLIC.as_bytes()),
        "a4afb63b56e811c9f4a681942385c2063ec19e63b99a56c1d680dd173009853a"
    );
}

#[test]
fn each_key_has_the_modulus_size_its_padding_path_needs() {
    // 校验公钥模长与填充格式
    assert_eq!(
        cipher::rsa_raw_encrypt_hex(b"{}", lyra_proto::consts::RSA_PUBLIC)
            .unwrap()
            .len(),
        128 * 2,
        "业务公钥是 1024 位"
    );
    let sid = cipher::rsa_oaep_sha256_base64(b"k", lyra_proto::consts::FP_RSA_PUBLIC).unwrap();
    assert_eq!(cipher::base64_decode(&sid).unwrap().len(), 256, "指纹公钥是 2048 位");
    // PKCS#1 v1.5 与无填充模幂对比
    let pkcs1 = cipher::rsa_pkcs1_encrypt_hex(b"{}", lyra_proto::consts::RSA_PUBLIC).unwrap();
    assert_eq!(pkcs1.len(), 256);
    assert_ne!(
        pkcs1,
        cipher::rsa_raw_encrypt_hex(b"{}", lyra_proto::consts::RSA_PUBLIC).unwrap()
    );
}

#[test]
fn krc_decode_matches_reference() {
    let blob = "a3JjMTjb0r5weMKMGfv45Jmq2N+ngpspRU/kT1FAmifV";
    assert_eq!(cipher::krc_decode_base64(blob).unwrap(), "<krc>hello 歌词</krc>");
}

// 设备与指纹测试

#[test]
fn mid_matches_reference() {
    assert_eq!(
        lyra_proto::mid_from_guid("e1f2a3b4c5d6e7f8091a2b3c4d5e6f70"),
        "93471393307406524540615984101025407175"
    );
}

#[test]
fn fingerprint_events_match_reference_byte_for_byte() {
    let mut rng = Mulberry32::new(FP_SEED);
    let draw = lyra_proto::Draw::take(&mut rng);
    let events = edt_events(&mut rng, draw.sentinel, draw.start, draw.end, draw.points);
    assert_eq!(events, GOLDEN_EVENTS.trim_end(), "轨迹与参考实现不一致");
}

#[test]
fn fingerprint_sid_and_edt_match_reference() {
    let mut rng = Mulberry32::new(FP_SEED);
    let sim = simulate(
        &mut rng,
        &SimInputs {
            mid: FP_MID,
            userid: FP_USERID,
            dfid: FP_DFID,
            webgl: FP_WEBGL,
            ts_millis: FP_TS,
        },
    )
    .unwrap();
    assert_eq!(sim.edt, GOLDEN_EDT.trim_end());
    // 校验 SID 长度与 Base64 解码
    assert_eq!(sim.sid.len(), GOLDEN_SID.trim_end().len());
    assert!(cipher::base64_decode(&sim.sid).is_ok());
    assert_eq!(cipher::base64_decode(&sim.sid).unwrap().len(), 256, "2048-bit 密钥");
}

//! 协议固定常量（AppId、版本号、签名盐值与密钥）。

/// 应用 ID
pub const APPID: u32 = 3116;
/// 客户端版本号
pub const CLIENTVER: u32 = 11440;
/// H5 侧应用 ID（预留常量，方案 §2.1；当前工作区未使用）
#[allow(dead_code)]
pub const SRAPPID: u32 = 2919;
/// 接口版本（预留常量，方案 §2.1；当前工作区未使用）
#[allow(dead_code)]
pub const APIVER: u32 = 20;

/// Android 签名盐值（兼用于云歌单 appkey 与 signParamsKey）
pub const SALT_MD5: &str = "LnT6xpN3khm36zse0QzvmgTZ3waWdRSA";
/// signKey 盐值
pub const SALT_SIGNKEY: &str = "185672dd44712f60bb1736df5a377e82";
/// Web 签名盐值
pub const SALT_WEB: &str = "NVPh5oo715z5DIWAeQlhMDsWXXQV4hwt";
/// 设备注册签名盐值
pub const SALT_REGISTER: &str = "1014";

/// 业务 RSA 公钥（无填充模幂）
pub const RSA_PUBLIC: &str = include_str!("../keys/lite_public.pem");
/// 指纹 RSA 公钥（OAEP-SHA256）
pub const FP_RSA_PUBLIC: &str = include_str!("../keys/lite_fingerprint.pem");

/// 微信开放平台 AppID（预留常量，方案 §2.1；当前工作区未使用）
#[allow(dead_code)]
pub const WX_APPID: &str = "wx72b795aca60ad321";
/// QQ 互联 AppID（预留常量，方案 §2.1；当前工作区未使用）
#[allow(dead_code)]
pub const QQ_APPID: &str = "101706348";

/// 取址 page_id
pub const PAGE_ID: u64 = 967_177_915;
/// 取址 ppage_id
pub const P_PAGE_ID: &str = "356753938,823673182,967485191";
/// 取址 pid
pub const PID: u32 = 411;
/// 搜索 version_code
pub const SEARCH_CLIENTVER: u32 = 201;

/// 默认 User-Agent
pub const UA_DEFAULT: &str = "Android15-1070-11083-46-0-DiscoveryDRADProtocol-wifi";
/// 短信登录 User-Agent
pub const UA_LOGIN: &str = "Android16-1070-11440-130-0-LOGIN-wifi";

/// 推荐标识头 kg-rc
pub const KG_RC: &str = "1";
/// 特征哈希头 kg-thash
pub const KG_THASH: &str = "5d816a0";
/// 推荐标识头 kg-rec
pub const KG_REC: &str = "1";
/// 风控校验头 kg-rf
pub const KG_RF: &str = "B9EDA08A64250DEFFBCADDEE00F8F25F";
/// 登录版本标识 gitversion
pub const GITVERSION: &str = "5f0b7c4";

/// 短信登录 t1 AES 密钥
pub const T1_KEY: &str = "5e4ef500e9597fe004bd09a46d8add98";
/// 短信登录 t1 AES IV
pub const T1_IV: &str = "04bd09a46d8add98";
/// 短信登录 t2 AES 密钥
pub const T2_KEY: &str = "fd14b35e3f81af3817a20ae7adae7020";
/// 短信登录 t2 AES IV
pub const T2_IV: &str = "17a20ae7adae7020";
/// 短信登录 t2 混淆片段
pub const T2_MIDDLE: &str = "0f607264fc6318a92b9e13c65db7cd3c";

/// 行为指纹 EDT IV
pub const FP_IV: &[u8; 16] = b"kugousecurity123";
/// KRC 歌词解密异或密钥
pub const KRC_KEY: [u8; 16] = [64, 71, 97, 119, 94, 50, 116, 71, 81, 54, 49, 45, 206, 210, 110, 105];

/// 大写字母数字字符集
pub const CHARSET_UPPER: &[u8; 36] = b"1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ";

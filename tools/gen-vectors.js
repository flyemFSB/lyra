
process.env.platform = 'lite';
const path = 'D:/YZB/KuGouMusicApi';
require('module').Module._initPaths();
const req = (m) => require(path + '/' + m);

const CryptoJS = req('node_modules/crypto-js');
const { cryptoMd5, cryptoAesEncrypt, cryptoAesDecrypt, cryptoRSAEncrypt, rsaEncrypt2, cryptoSha1 } = req('util/crypto');
const { signatureAndroidParams, signatureWebParams, signatureRegisterParams, signKey, signParamsKey } = req('util/helper');
const { calculateMid, getGuid, decodeLyrics } = req('util/util');

const out = {};

// 签名计算向量。
const p1 = { dfid: '-', mid: '12345678901234567890', uuid: '-', appid: 3116, clientver: 11440, clienttime: 1750000000, token: 'TOKEN123', userid: 2043841403 };
out.sig_android_p1_body = signatureAndroidParams(p1, JSON.stringify({ listid: 2, userid: 2043841403 }));
out.sig_android_p1_empty = signatureAndroidParams(p1, '');
out.sig_web_p1 = signatureWebParams(p1, JSON.stringify({ a: 1 }));

const p2 = { keyword: '周杰伦 🎵', page: 1, pagesize: 30, clientver: 201, area_code: 1, dopicfull: 1, platform: 'AndroidFilter', iscorrection: 1, privilegefilter: 0 };
out.sig_android_p2 = signatureAndroidParams(p2, '');

const p3 = { a: 1, obj: { z: 1, a: [1, 2, { q: 'x' }] }, s: 'v' };
out.sig_android_p3 = signatureAndroidParams(p3, '');

out.sig_register = signatureRegisterParams({ dfid: 'ABC', mid: '99', uuid: '-', platid: 1, part: 1 });
out.sign_key = signKey('HASHABCDEF0123456789', '12345678901234567890', 2043841403, 3116);
out.sign_params_key = signParamsKey(1750000000000);

// AES 加解密向量。
// 指定 key 时直接输出十六进制密文字符串。
out.aes_derived = cryptoAesEncrypt({ mobile: '13800138000', code: '123456' }, { key: 'abcdefghijklmnop' });
out.aes_derived_back = cryptoAesDecrypt(out.aes_derived, 'abcdefghijklmnop');
out.t1 = cryptoAesEncrypt('|1750000000000', { key: '5e4ef500e9597fe004bd09a46d8add98', iv: '04bd09a46d8add98' });
out.t2 = cryptoAesEncrypt(
  'e1f2a3b4c5d6e7f8091a2b3c4d5e6f70|0f607264fc6318a92b9e13c65db7cd3c|02:00:00:00:00:00|ABCDEFGHIJ|1750000000000',
  { key: 'fd14b35e3f81af3817a20ae7adae7020', iv: '17a20ae7adae7020' });
out.aes_derived2 = cryptoAesEncrypt('中文 payload 🎵', { key: 'zzzz' });

// 云端歌单固定密钥加密。
function cloudEncrypt(plain, secret) {
  const md5 = cryptoMd5(secret);
  const key = md5.substring(0, 16), iv = md5.substring(16, 32);
  const c = CryptoJS.AES.encrypt(CryptoJS.enc.Utf8.parse(plain), CryptoJS.enc.Utf8.parse(key),
    { iv: CryptoJS.enc.Utf8.parse(iv), mode: CryptoJS.mode.CBC, padding: CryptoJS.pad.Pkcs7 });
  return CryptoJS.enc.Base64.stringify(c.ciphertext);
}
out.cloud = cloudEncrypt(JSON.stringify({ aes: 'Ab3xZ9', uid: 2043841403, token: 'T' }), '123456');
out.cloud_plain = JSON.stringify({ aes: 'Ab3xZ9', uid: 2043841403, token: 'T' });

// RSA 确定性加密向量。
out.rsa_raw = cryptoRSAEncrypt({ token: 'T', clienttime: 1750000000 });
out.rsa_raw_len = out.rsa_raw.length;
out.rsa_pkcs1 = rsaEncrypt2({ aes: 'Ab3xZ9', uid: 2043841403, token: 'T' });

// 设备身份标识计算。
out.mid = calculateMid('e1f2a3b4c5d6e7f8091a2b3c4d5e6f70');
out.guid_hash = cryptoMd5('550e8400-e29b-41d4-a716-446655440000');
out.mid2 = calculateMid(out.guid_hash);
out.sha1 = cryptoSha1('lyra');

// KRC 歌词解密向量。
const pako = req('node_modules/pako');
const plain = '<krc>hello 歌词</krc>';
const deflated = pako.deflate(new TextEncoder().encode(plain));
const enKey = [64, 71, 97, 119, 94, 50, 116, 71, 81, 54, 49, 45, 206, 210, 110, 105];
for (let i = 0; i < deflated.length; i++) deflated[i] ^= enKey[i % enKey.length];
const blob = Buffer.concat([Buffer.from([0x6b, 0x72, 0x63, 0x31]), Buffer.from(deflated)]);
out.krc_b64 = blob.toString('base64');
out.krc_plain = decodeLyrics(blob.toString('base64'));

// 行为指纹仿真数据生成。
const { generateSimulate } = req('util/generate_simulate');
function mulberry32(a) { return function () { a = (a + 0x6D2B79F5) | 0; let t = a; t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296; }; }
const rng = mulberry32(0xC0FFEE);
Math.random = rng;
Date.now = () => 1750000000000;
let captured = null;
const origLog = console.log;
console.log = (s) => { captured = s; };
// 注入固定 WebGL 参数以保证结果可复现。
const sim = generateSimulate('12345678901234567890', '2043841403', 'abcdef1234', '9876543210987654321');
console.log = origLog;
out.fp_plaintext = captured;
out.fp_edt = sim.edt;
out.fp_sid = sim.sid;
out.fp_key = out.fp_plaintext.slice(0, 0);
out.fp_meta = { edt_len: sim.edt.length, sid_len: sim.sid.length };

console.log(JSON.stringify(out, null, 1));

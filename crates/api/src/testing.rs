//! 单元测试辅助模块：构造测试用设备身份、会话与客户端实例。
#![cfg(test)]

use std::sync::Arc;

use crate::client::Api;
use crate::spec::{Account, Session};
use crate::transport::MockTransport;

/// 构造测试用固定设备身份。
pub(crate) fn device(dfid: &str) -> lyra_proto::Device {
    lyra_proto::Device {
        guid: "e8456cee7fd2348cfa239342b10a23bd".to_owned(),
        mid: "12345678901234567890".to_owned(),
        mac: "02:00:00:00:00:00".to_owned(),
        dev: "ABCDEFGHIJ".to_owned(),
        webgl: "1".to_owned(),
        dfid: dfid.to_owned(),
    }
}

/// 已登录会话。
pub(crate) fn session() -> Session {
    Session::new(device("DfidValue")).with_account(Account {
        userid: 2_043_841_403,
        token: "TOKEN123".to_owned(),
        vip_type: 0,
        vip_token: String::new(),
    })
}

/// 构造未登录匿名会话。
pub(crate) fn anonymous() -> Session {
    Session::new(device("-"))
}

/// 已登录的 `Api`。
pub(crate) fn api(mock: Arc<MockTransport>) -> Api {
    Api::new(mock, session(), 64)
}

/// 未登录的 `Api`。
pub(crate) fn anonymous_api(mock: Arc<MockTransport>) -> Api {
    Api::new(mock, anonymous(), 64)
}

/// 构造指定设备标识的测试客户端。
pub(crate) fn api_with_dfid(mock: Arc<MockTransport>, dfid: &str) -> Api {
    Api::new(mock, Session::new(device(dfid)), 64)
}

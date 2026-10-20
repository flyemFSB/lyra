//! 构建脚本：在 Windows 平台嵌入应用图标。

#![allow(clippy::unwrap_used)]

fn main() {
    println!("cargo:rerun-if-changed=res/lyra.rc");
    println!("cargo:rerun-if-changed=res/logo.ico");

    #[cfg(target_os = "windows")]
    {
        // GPUI（gpui-pre 的 windows-manifest 特性）已经向最终可执行文件嵌入了
        // 一份 RT_MANIFEST 资源。这里只补图标：重复定义清单会导致链接期
        // CVT1100「资源重复」而失败。
        embed_resource::compile(
            "res/lyra.rc",
            embed_resource::ParamsIncludeDirs([std::path::Path::new("res")]),
        )
        .manifest_optional()
        .unwrap();
    }
}

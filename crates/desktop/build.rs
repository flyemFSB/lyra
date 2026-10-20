//! 构建脚本：在 Windows 平台嵌入应用图标与 GPUI 运行时清单。

#![allow(clippy::unwrap_used)]

fn main() {
    println!("cargo:rerun-if-changed=res/lyra.rc");
    println!("cargo:rerun-if-changed=res/logo.ico");
    println!("cargo:rerun-if-changed=res/gpui.manifest.xml");

    #[cfg(target_os = "windows")]
    {
        embed_resource::compile("res/lyra.rc", embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }
}

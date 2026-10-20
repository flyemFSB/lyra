//! 构建脚本：在 Windows 平台嵌入应用图标资源。

fn main() {
    println!("cargo:rerun-if-changed=res/lyra.rc");
    println!("cargo:rerun-if-changed=res/logo.ico");

    #[cfg(target_os = "windows")]
    {
        let Ok(out_dir) = std::env::var("OUT_DIR") else { return };
        let res_o = format!("{out_dir}/lyra_res.o");
        let rc_file = "res/lyra.rc";
        if std::path::Path::new(rc_file).exists() {
            let status = std::process::Command::new("windres")
                .args(["-i", rc_file, "-o", &res_o])
                .status();
            if let Ok(s) = status
                && s.success()
            {
                println!("cargo:rustc-link-arg={res_o}");
            }
        }
    }
}

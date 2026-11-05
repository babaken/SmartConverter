//! Windows 向けに、実行ファイルへアイコンとマニフェストを埋め込む。

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon/smartconverter.ico");
    println!("cargo:rerun-if-changed=../../assets/windows/smartconverter.manifest");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon/smartconverter.ico");
        res.set_manifest_file("../../assets/windows/smartconverter.manifest");
        if let Err(e) = res.compile() {
            // リソースコンパイラが無い環境でもビルドは続ける（アイコンなしの exe になる）。
            println!("cargo:warning=アイコンを埋め込めませんでした: {e}");
        }
    }
}

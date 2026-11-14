//! 把图标与版本信息编进 exe。
//!
//! 只做一件事：用 `embed-resource` 调 Windows SDK 的 `rc.exe`，把
//! `assets/app.rc` 编译成 `.res` 再交给链接器。`rc.exe` 不在 PATH 里也没关系，
//! `embed-resource` 会自己到 Windows Kits 与 Visual Studio 的常见位置去找。

fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");

    // `.rc` 里没有清单资源，所以要显式告诉 embed-resource：清单可以缺失。
    // （3.x 必须二选一调用，否则 compile 直接 panic。）
    embed_resource::compile("assets/app.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("编译 app.rc");
}

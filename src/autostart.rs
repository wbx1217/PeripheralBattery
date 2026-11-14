//! 开机自启（写 HKCU 的 Run 键）。
//!
//! 不用计划任务、不用服务 —— 用户级 Run 键就够，卸载时删值即可，也不需要管理员权限。

use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

/// Run 键下的值名。
///
/// 任务管理器「启动」页显示的就是这个值名，所以和软件名保持一致。
const VALUE_NAME: &str = "外设电量显示";
/// 改名之前用的值名。
///
/// 留着只为了迁移：旧条目里记的是 `peripheral-battery.exe`，那个文件已经不存在了，
/// 不清理的话用户会在「启动」页看到一条永远起不来的项。
const LEGACY_VALUE_NAME: &str = "PeripheralBattery";
/// Run 键路径。
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// 当前是否已设置开机自启。
#[must_use]
pub fn is_enabled() -> bool {
    exists(VALUE_NAME) || exists(LEGACY_VALUE_NAME)
}

/// 把旧值名迁到新值名。
///
/// 只在「旧的有、新的没有」时动手：用户若已经手动改过新值，就别覆盖他。
pub fn migrate_legacy() {
    if exists(LEGACY_VALUE_NAME) && !exists(VALUE_NAME) {
        let _ = set_enabled(true);
    }
}

/// 设置或取消开机自启。
///
/// # Errors
///
/// 注册表打不开或写入失败时返回系统的错误码。
pub fn set_enabled(enabled: bool) -> Result<(), u32> {
    // 打开失败时把系统错误码原样带出去：1（我们自己的哨兵）分不清「没权限」和「键不存在」。
    let key = open_key(KEY_SET_VALUE)?;
    let name = wide(VALUE_NAME);
    let status = if enabled {
        write(key, &name)?
    } else {
        delete(key, &name)
    };

    // 旧值名无论开还是关都清掉：它指向的可执行文件已经改过名了。
    let legacy = wide(LEGACY_VALUE_NAME);
    // SAFETY: key 由 open_key 成功打开且带 KEY_SET_VALUE，legacy 以 NUL 结尾。
    unsafe { RegDeleteValueW(key, legacy.as_ptr()) };
    // SAFETY: 与 open_key 配对。
    unsafe { RegCloseKey(key) };

    // 「删一个本来就没有的值」算成功；「写」失败就是失败，不能因为键不存在就放过。
    if status == ERROR_SUCCESS || (!enabled && status == ERROR_FILE_NOT_FOUND) {
        Ok(())
    } else {
        Err(status)
    }
}

/// 某个值名下是否有数据（只问大小，不读内容）。
fn exists(name: &str) -> bool {
    let Ok(key) = open_key(KEY_QUERY_VALUE) else {
        return false;
    };
    let name = wide(name);
    let mut kind = 0u32;
    let mut size = 0u32;

    // SAFETY: key 有效，name 以 NUL 结尾；数据指针传空表示只查询所需大小。
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            std::ptr::null_mut(),
            &mut size,
        )
    };
    // SAFETY: key 是本次打开的有效句柄，用完即关。
    unsafe { RegCloseKey(key) };
    status == ERROR_SUCCESS
}

/// 把「当前 exe 的完整路径」写进 Run 键。
fn write(key: HKEY, name: &[u16]) -> Result<u32, u32> {
    // 拿不到自己的路径就别写：写个空串进 Run 键，开机时只会弹一个找不到文件的错误。
    let exe = std::env::current_exe().map_err(|_| ERROR_FILE_NOT_FOUND)?;
    // 路径可能带空格（安装在 Program Files 之类），必须带引号。
    let command = format!("\"{}\"", exe.display());
    let data = wide(&command);
    // SAFETY: key 有效，name/data 都以 NUL 结尾，长度按字节给出。
    let status = unsafe {
        RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr().cast::<u8>(),
            u32::try_from(data.len() * 2).unwrap_or(0),
        )
    };
    Ok(status)
}

fn delete(key: HKEY, name: &[u16]) -> u32 {
    // SAFETY: key 有效，name 以 NUL 结尾。
    unsafe { RegDeleteValueW(key, name.as_ptr()) }
}

fn open_key(access: u32) -> Result<HKEY, u32> {
    let path = wide(RUN_KEY);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: path 以 NUL 结尾，key 是出参，成功时才关闭。
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, access, &mut key) };
    if status == ERROR_SUCCESS { Ok(key) } else { Err(status) }
}

fn wide(text: &str) -> Vec<u16> {
    OsString::from(text)
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

//! 用户偏好：注册表里的一小块自留地。
//!
//! 只有几个开关（主题、刷新间隔），犯不上为它引入配置文件的读写流程。
//! 键在 `HKCU\Software\外设电量显示`，值名直接用人能读的单词。

use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

/// 本软件自己的配置键。
const PREFS_KEY: &str = r"Software\外设电量显示";

/// 主题：亮 / 暗两态。
///
/// 刻意不做「跟随系统」：这是个点开看一眼就收起的托盘挂件，出现时该长什么样
/// 应当稳定可预期，跟着系统在用户没注意的时候换色反而是干扰。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ThemeMode {
    /// 亮色（默认）。
    #[default]
    Light,
    /// 暗色。
    Dark,
}

impl ThemeMode {
    /// 读回字符串形式，供持久化用。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// 从持久化的字符串还原；不认识的一律当亮色。
    ///
    /// 老版本写过 `"auto"`（跟随系统）。这里没有 `Auto` 分支，它会落到默认的
    /// 亮色 —— 正是我们要的迁移行为，不需要额外写迁移代码。
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "dark" => Self::Dark,
            _ => Self::Light,
        }
    }

    /// 在两态之间切换。
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::Light,
        }
    }

    /// 这一帧要不要用暗色。
    #[must_use]
    pub fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }
}

/// 读出主题偏好。
#[must_use]
pub fn theme_mode() -> ThemeMode {
    ThemeMode::parse(&read(PREFS_KEY, "Theme").unwrap_or_default())
}

/// 写入主题偏好。失败就算了 —— 下次启动回到跟随系统，不影响使用。
pub fn set_theme_mode(mode: ThemeMode) {
    let _ = write(PREFS_KEY, "Theme", mode.as_str());
}

/// 读出上次选的刷新间隔（毫秒）。没存过或类型不对就返回 `None`。
#[must_use]
pub fn interval_ms() -> Option<u64> {
    read_u32(PREFS_KEY, "IntervalMs").map(u64::from)
}

/// 记住刷新间隔。
pub fn set_interval_ms(value: u64) {
    let _ = write_u32(PREFS_KEY, "IntervalMs", u32::try_from(value).unwrap_or(0));
}

/// 读一个 DWORD 值。
fn read_u32(key: &str, name: &str) -> Option<u32> {
    let key = open_path(key, KEY_READ)?;
    let name = wide(name);
    let mut kind = 0u32;
    let mut value = 0u32;
    let mut size = u32::try_from(std::mem::size_of::<u32>()).unwrap_or(4);

    // SAFETY: key 有效且带 KEY_READ，name 以 NUL 结尾；value/size 都是本栈上的出参。
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            std::ptr::from_mut(&mut value).cast::<u8>(),
            &mut size,
        )
    };
    // SAFETY: 与 open_path 配对。
    unsafe { RegCloseKey(key) };
    if status != ERROR_SUCCESS || kind != REG_DWORD || size != 4 {
        return None;
    }
    Some(value)
}

/// 写一个 DWORD 值。
fn write_u32(key: &str, name: &str, value: u32) -> Result<(), u32> {
    let key = create_path(key)?;
    let name = wide(name);

    // SAFETY: key 有效且带 KEY_SET_VALUE，name 以 NUL 结尾，data 指向 4 字节的 u32。
    let status = unsafe {
        RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            REG_DWORD,
            std::ptr::from_ref(&value).cast::<u8>(),
            4,
        )
    };
    // SAFETY: 与 create_path 配对。
    unsafe { RegCloseKey(key) };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(status)
    }
}

/// 读一个字符串值。
fn read(key: &str, name: &str) -> Option<String> {
    let key = open_path(key, KEY_READ)?;
    let name = wide(name);
    let mut kind = 0u32;
    let mut size = 0u32;

    // SAFETY: key 有效且带 KEY_READ，name 以 NUL 结尾；数据指针传空只查询长度。
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
    if status != ERROR_SUCCESS || size == 0 {
        // SAFETY: 与 open_path 配对。
        unsafe { RegCloseKey(key) };
        return None;
    }

    // 注册表里的字符串是 UTF-16，长度按字节给（可能含结尾的 NUL）。
    let mut buffer = vec![0u8; size as usize];
    // SAFETY: buffer 有 size 字节，key/name 均有效。
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            buffer.as_mut_ptr(),
            &mut size,
        )
    };
    // SAFETY: 同上。
    unsafe { RegCloseKey(key) };
    if status != ERROR_SUCCESS {
        return None;
    }
    if kind != REG_SZ {
        // 类型对不上就当没写过：拿 DWORD 的字节当 UTF-16 解出来的是垃圾。
        return None;
    }

    let units: Vec<u16> = buffer
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|unit| *unit != 0)
        .collect();
    String::from_utf16(&units).ok()
}

/// 写一个字符串值，键不存在就创建。
fn write(key: &str, name: &str, value: &str) -> Result<(), u32> {
    let key = create_path(key)?;
    let name = wide(name);
    let data = wide(value);

    // SAFETY: key 有效且带 KEY_SET_VALUE，name/data 以 NUL 结尾，长度按字节给出。
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
    // SAFETY: 与 create_path 配对。
    unsafe { RegCloseKey(key) };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(status)
    }
}

fn open_path(path: &str, access: u32) -> Option<HKEY> {
    let path = wide(path);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: path 以 NUL 结尾，key 是出参，只有成功时才需要关闭。
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, access, &mut key) };
    (status == ERROR_SUCCESS).then_some(key)
}

fn create_path(path: &str) -> Result<HKEY, u32> {
    let path = wide(path);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: path 以 NUL 结尾，key 是出参；不关心 disposition。
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    if status == ERROR_SUCCESS {
        Ok(key)
    } else {
        Err(status)
    }
}

fn wide(text: &str) -> Vec<u16> {
    OsString::from(text)
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::ThemeMode;

    /// 切换一次到暗、再切一次回到亮 —— 两态必须闭合。
    #[test]
    fn theme_mode_toggles_between_two_states() {
        assert_eq!(ThemeMode::Light.next(), ThemeMode::Dark);
        assert_eq!(ThemeMode::Dark.next(), ThemeMode::Light);
        assert_eq!(ThemeMode::default(), ThemeMode::Light);
    }

    /// 持久化用的字符串必须能原样读回来。
    #[test]
    fn theme_mode_survives_round_trip() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            assert_eq!(ThemeMode::parse(mode.as_str()), mode);
        }
    }

    /// 老版本写下的 `"auto"` 与空值都归到默认亮色。
    #[test]
    fn legacy_values_fall_back_to_light() {
        assert_eq!(ThemeMode::parse("auto"), ThemeMode::Light);
        assert_eq!(ThemeMode::parse(""), ThemeMode::Light);
        assert_eq!(ThemeMode::parse("DarkMode"), ThemeMode::Light);
    }

    /// 配色只能由模式决定，不依赖任何系统状态。
    #[test]
    fn dark_flag_matches_mode() {
        assert!(!ThemeMode::Light.is_dark());
        assert!(ThemeMode::Dark.is_dark());
    }
}

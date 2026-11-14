//! 协议探测工具。
//!
//! 这不是产品的一部分，只用来回答两个问题：设备暴露了哪些集合、它们对哪种
//! 报文有反应。用法：
//!
//! ```text
//! probe list                              # 列出全部 HID 集合
//! probe list --vid 3654                   # 只看某厂商
//! probe listen --vid 3654 --secs 20        # 被动监听，只读打开
//! probe send --vid 3654 --pid 6901 --usage-page FF00 --hex "02 80 09" --mode setout
//! ```

use std::process::ExitCode;
use std::time::{Duration, Instant};

use peripheral_battery::hid::{enumerate, session::hex, HidDevice, HidSession};
use peripheral_battery::HidError;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("错误：{err}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), HidError> {
    let Some(command) = args.first() else {
        return Err(HidError::Usage(usage_text()));
    };

    match command.as_str() {
        "list" => list(&args[1..]),
        "listen" => listen(&args[1..]),
        "send" => send(&args[1..]),
        "help" | "--help" | "-h" => {
            println!("{}", usage_text());
            Ok(())
        }
        other => Err(HidError::Usage(format!("未知子命令 `{other}`\n{}", usage_text()))),
    }
}

fn usage_text() -> String {
    [
        "用法：",
        "  probe list [--vid HEX] [--pid HEX]",
        "  probe listen [--vid HEX] [--pid HEX] [--usage-page HEX] [--secs N] [--writable]",
        "  probe send --vid HEX --pid HEX [--usage-page HEX] --hex \"AA BB CC\" [--mode write|setout|setfeat|getfeat] [--wait MS]",
    ]
    .join("\n")
}

/// 读取 `--name value` 形式的参数。
fn arg_value(args: &[String], name: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == name)?;
    args.get(index + 1).cloned()
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| arg == name)
}

/// 解析十六进制参数，`--vid` / `--pid` 这类单值用。
fn parse_hex_u16(args: &[String], name: &str) -> Result<Option<u16>, HidError> {
    let Some(raw) = arg_value(args, name) else {
        return Ok(None);
    };
    let cleaned = raw.trim().trim_start_matches("0x").trim_start_matches("0X");
    u16::from_str_radix(cleaned, 16)
        .map(Some)
        .map_err(|_| HidError::Usage(format!("{name} 需要十六进制数值，收到 `{raw}`")))
}

/// 解析 `"02 80 09"` 这样的报文。
fn parse_hex_bytes(args: &[String]) -> Result<Vec<u8>, HidError> {
    let raw = arg_value(args, "--hex")
        .ok_or_else(|| HidError::Usage("缺少 --hex".to_owned()))?;
    let mut out = Vec::new();
    for token in raw.split_whitespace() {
        let cleaned = token.trim_start_matches("0x").trim_start_matches("0X");
        let byte = u8::from_str_radix(cleaned, 16)
            .map_err(|_| HidError::Usage(format!("非法字节 `{token}`")))?;
        out.push(byte);
    }
    if out.is_empty() {
        return Err(HidError::Usage("--hex 为空".to_owned()));
    }
    Ok(out)
}

/// 按 VID / PID / usage page 过滤设备。
fn filter(devices: Vec<HidDevice>, args: &[String]) -> Result<Vec<HidDevice>, HidError> {
    let vid = parse_hex_u16(args, "--vid")?;
    let pid = parse_hex_u16(args, "--pid")?;
    let usage_page = parse_hex_u16(args, "--usage-page")?;
    Ok(devices
        .into_iter()
        .filter(|device| vid.is_none_or(|want| device.vendor_id == want))
        .filter(|device| pid.is_none_or(|want| device.product_id == want))
        .filter(|device| usage_page.is_none_or(|want| device.usage_page == want))
        .collect())
}

fn list(args: &[String]) -> Result<(), HidError> {
    let devices = filter(enumerate()?, args)?;
    println!("共 {} 个集合", devices.len());
    for (index, device) in devices.iter().enumerate() {
        let product = if device.product.is_empty() {
            "-"
        } else {
            device.product.as_str()
        };
        println!(
            "[{index}] {:04X}:{:04X} v{:04X} UP={:04X} U={:04X} IN={} OUT={} FEA={}  {}",
            device.vendor_id,
            device.product_id,
            device.version,
            device.usage_page,
            device.usage,
            device.input_len,
            device.output_len,
            device.feature_len,
            product
        );
        println!("     {}", device.path);
    }
    Ok(())
}

fn listen(args: &[String]) -> Result<(), HidError> {
    let devices = filter(enumerate()?, args)?;
    if devices.is_empty() {
        return Err(HidError::Usage("没有匹配的设备".to_owned()));
    }
    let writable = has_flag(args, "--writable");
    let seconds = arg_value(args, "--secs")
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(15);

    let mut sessions = Vec::new();
    for (index, device) in devices.iter().enumerate() {
        println!(
            "[{index}] {:04X}:{:04X} UP={:04X} IN={} OUT={} FEA={} {}",
            device.vendor_id,
            device.product_id,
            device.usage_page,
            device.input_len,
            device.output_len,
            device.feature_len,
            device.product
        );
        match HidSession::open(&device.path, writable)
            .map(|session| {
                session.with_report_lengths(device.input_len, device.output_len, device.feature_len)
            }) {
            Ok(session) => sessions.push((index, device.clone(), session)),
            Err(err) => eprintln!("[{index}] 打不开，跳过：{err}"),
        }
    }
    if sessions.is_empty() {
        return Err(HidError::Usage("所有设备都打不开".to_owned()));
    }

    println!("监听 {seconds} 秒，Ctrl+C 结束...");
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        let mut got_any = false;
        for (index, device, session) in &sessions {
            match session.read(80) {
                Ok(Some(report)) => {
                    got_any = true;
                    println!(
                        "[{index}] {:04X}:{:04X} UP={:04X} len={} | {}",
                        device.vendor_id,
                        device.product_id,
                        device.usage_page,
                        report.len(),
                        hex(&report)
                    );
                }
                Ok(None) => {}
                Err(err) => eprintln!("[{index}] 读取失败：{err}"),
            }
        }
        if !got_any {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    Ok(())
}

fn send(args: &[String]) -> Result<(), HidError> {
    let devices = filter(enumerate()?, args)?;
    if devices.is_empty() {
        return Err(HidError::Usage("没有匹配的设备".to_owned()));
    }
    let mode = arg_value(args, "--mode").unwrap_or_else(|| "setout".to_owned());
    let frame = parse_hex_bytes(args)?;
    let wait = arg_value(args, "--wait")
        .and_then(|raw| raw.parse::<u32>().ok())
        .unwrap_or(600);
    let repeat = arg_value(args, "--repeat")
        .and_then(|raw| raw.parse::<u32>().ok())
        .unwrap_or(1);

    for (index, device) in devices.iter().enumerate() {
        println!(
            "[{index}] {:04X}:{:04X} UP={:04X} IN={} OUT={} FEA={} {}",
            device.vendor_id,
            device.product_id,
            device.usage_page,
            device.input_len,
            device.output_len,
            device.feature_len,
            device.product
        );
        let session = HidSession::open(&device.path, true).map(|session| {
            session.with_report_lengths(device.input_len, device.output_len, device.feature_len)
        });
        let session = match session {
            Ok(session) => session,
            Err(err) => {
                eprintln!("[{index}] 打不开，跳过：{err}");
                continue;
            }
        };

        let padded = pad_to(&frame, session.output_len());
        println!("    发送 {} | {}", mode, hex(&padded));

        for round in 0..repeat.max(1) {
            session.flush(30);
            match mode.as_str() {
                "write" => session.write(&padded, wait)?,
                "setfeat" => session.set_feature(&padded)?,
                "getfeat" => {
                    let mut buffer = padded.clone();
                    session.get_feature(&mut buffer)?;
                    println!("    特性报文回复 | {}", hex(&buffer));
                    continue;
                }
                _ => session.set_output_report(&padded)?,
            }

            let deadline = Instant::now() + Duration::from_millis(u64::from(wait));
            while Instant::now() < deadline {
                match session.read(120)? {
                    Some(report) => println!("    [{round}] 回复 | {}", hex(&report)),
                    None => break,
                }
            }
            if repeat > 1 {
                std::thread::sleep(Duration::from_millis(150));
            }
        }
    }
    Ok(())
}

/// 把报文补零到设备声明的长度：多数 Windows HID 接口要求长度严格匹配。
fn pad_to(frame: &[u8], len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len.max(frame.len())];
    out[..frame.len()].copy_from_slice(frame);
    out
}

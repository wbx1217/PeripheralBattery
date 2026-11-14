# 实测协议（两台设备，均已在真机验证）

验证环境：Windows 11，2026-11-12。所有报文都是本机抓到的原始字节，不是推测。

---

## 1. 迈从 G87 键盘 —— `41E4:2001`，产品名 `MCHOSE 2.4G Wireless`

| 项 | 值 |
|---|---|
| 集合 | Usage Page `0xFF02`，Usage `0x0002`，IN 20 / OUT 20，无 Feature Report |
| 通道 | `HID SET_REPORT`（控制管道）。interrupt OUT 无应答 |
| 帧长 | 20 字节，`frame[0]` 即 report id `0x13`（不是额外的前缀）|

### 帧格式

```
frame[0]     = 0x13   report id
frame[1]     = 命令    0x4A = 查询电量
frame[2..3]  = 01 00
frame[4..18] = 载荷
frame[19]    = 校验和 = sum(frame[0..18]) & 0xFF
```

### 实测收发

```
查询  13 4A 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 5E
回复  13 4A 01 00 02 63 01 00 00 00 00 00 00 00 00 00 00 00 00 C4
                  ^^ ^^
                  |  +---- frame[5] = 0x63 = 99%
                  +------- frame[4] = 02 = 后续载荷长度
```

- `frame[6]` 低半字节 = fullStatus，高半字节 = chargeStatus（`1` = 充电中）
- 回复必须校验：`frame[2]==0x01 && frame[3]==0 && frame[4]==0x02 && frame[19]==checksum && frame[5]<=100`。
  接收器会先把查询帧原样回显一遍（`frame[4]==0`），必须跳过，否则会把查询帧当成 0% 读取。

> 这套帧和 Voltide 里 MCHOSE K99 v2 的 20 字节帧完全一致 —— 换了 VID/PID，协议没变。
> 注意本机 VID 是 `41E4`，不是 Voltide 记录的 `3554` / `3837`。

---

## 2. DARKRIM SHADOW 耳机 —— `3654:6901`（杰理 JieLi 方案 dongle）

| 项 | 值 |
|---|---|
| 集合 | Usage Page `0xFF02`，Usage `0x0002`，IN 65 / OUT 65，无 Feature Report |
| 通道 | interrupt OUT（`WriteFile`）。控制管道写会返回错误 87 |
| 帧长 | 65 字节，`frame[0]` 固定 `0x00`，无 report id |
| 芯片 | 耳机固件里出现 `Jie:` 字样；厂商 DLL `libdongle.dll` 含 `RcspCommand` 类 |

### 帧格式

```
frame[0]      = 0x00
frame[1]      = 0xFE   帧头
frame[2]      = 序号（每次请求 +1）
frame[3]      = 0x10 本机侧 / 0x11 耳机侧
frame[4]      = 0x00
frame[5]      = 0x02 / 0x03
frame[6]      = 分片序号
frame[7]      = 0x00 单帧请求；非零表示后面还有分片
frame[8..9]   = 载荷长度（小端）
frame[10..]   = 载荷（TLV：type(1) + len(1) + value(len)）
frame[10+len] = CRC16 高字节
frame[11+len] = CRC16 低字节
```

**CRC = CRC16-CCITT-FALSE（poly `0x1021`，init `0xFFFF`，不反转），
覆盖 `frame[1 .. 10+len)`，大端存放。**

### TLV 类型

| type | 含义 |
|---|---|
| `0x01` | 状态 / 电量（value[0] = 百分比） |
| `0x02` | 设备名（`DARKRIM SHADOW`） |
| `0x03` | EQ 数据（多分片） |
| `0x04` | dongle 固件版本 |
| `0x05` | 耳机固件版本 |
| `0x06` | 状态码 |
| `0x09` | 基础信息 |
| `0x0B` | 地址 / ID（6 字节） |
| `0xFF` | 校验信息 |

### 实测收发（查询电量 = 发 TLV `0x01`，长度 0）

```
查询 00 FE 00 10 00 02 00 00 02 00 01 00 58 09 00 ...
回复 00 FE 00 10 00 02 00 00 03 00 01 01 50 11 68 00 ...
                              ^^ ^^ ^^ ^^ ^^^^^
                              |  |  |  |  +---- CRC16 = 0x1168
                              |  |  |  +------- TLV 0x01, len 1, value 0x50 = 80%
                              |  |  +---------- 载荷长度 3（小端）
                              +--+------------- 长度字段
```

### 状态判定

- 耳机没开机 / 没连上时，dongle **完全不应答**（不是回错误，是沉默）。UI 要区分「休眠」与「故障」。

---

## 不在本工具范围内

| 设备 | 说明 |
|---|---|
| `373B:1115` `ATK A9 Plus Nearlink` | 星闪鼠标，用户当前需求只要键盘 + 耳机，故不实现 |

---

## 探测工具

`probe` 子命令（`cargo run --bin probe -- ...`）保留在仓库里，用于将来接入新设备：

```bash
probe list                                  # 列出全部 HID 集合
probe list --vid 3654                       # 只看某厂商
probe listen --vid 3654 --secs 15           # 被动监听（只读打开）
probe send --vid 3654 --pid 6901 --usage-page FF02 --hex "00 FE ..." --mode write
```

# GetSMART

GetSMART は、Windows と Linux でストレージデバイスの SMART 情報を取得する Rust ライブラリ/CLI/FFI 提供プロジェクトです。

- Rust API: デバイス一覧取得と SMART レポート取得
- CLI: JSON での簡易確認
- C FFI: `cdylib` 経由で他言語から利用可能

## 主な機能

- 対応プロトコル
  - ATA
  - NVMe
- 出力形式
  - `serde` 対応構造体
  - FFI では JSON エンベロープ
- エラーハンドリング
  - 統一エラーコード (`invalid_argument`, `permission_denied` など)

## 対応プラットフォーム

- Windows
- Linux

上記以外の OS では `unsupported_platform` が返ります。

## 必要要件

- Rust (stable)
- Cargo

OS ごとの実行権限:

- Windows: 物理ドライブアクセス権限が必要です（管理者権限推奨）
- Linux: `/dev/*` への read/write と ioctl 実行権限が必要です（通常は root または適切な権限設定）

## ビルド

ライブラリと CLI をビルド:

```bash
cargo build
```

リリースビルド:

```bash
cargo build --release
```

## CLI の使い方

### 1) デバイス一覧

```bash
cargo run -- list
```

`DeviceInfo` 配列を JSON で出力します。

### 2) SMART 情報取得

```bash
cargo run -- read <device_id>
```

例:

- Windows: `cargo run -- read physicaldrive:0`
- Linux NVMe: `cargo run -- read nvme:nvme0`
- Linux ATA: `cargo run -- read ata:sda`

## Rust ライブラリとして利用

`Cargo.toml` 例:

```toml
[dependencies]
getsmart = { package = "GetSMART", path = "." }
```

利用例:

```rust
use getsmart::{get_smart, list_devices};

fn main() {
    let devices = list_devices().expect("failed to list devices");
    println!("devices: {}", devices.len());

    if let Some(first) = devices.first() {
        let report = get_smart(&first.id).expect("failed to read SMART");
        println!("{} -> {:?}", report.device.id, report.summary.passed);
    }
}
```

## C FFI として利用

公開ヘッダは `include/getsmart.h` です。

```c
#ifndef GETSMART_H
#define GETSMART_H

char* getsmart_list_devices_json(void);
char* getsmart_get_smart_json(const char* device_id);
void getsmart_free_string(char* ptr);
const char* getsmart_version(void);

#endif
```

FFI の戻り値は UTF-8 JSON 文字列です。呼び出し側で `getsmart_free_string` による解放が必要です。

### FFI の JSON 形式

成功:

```json
{
  "ok": true,
  "data": { ... }
}
```

失敗:

```json
{
  "ok": false,
  "error": {
    "code": "invalid_argument",
    "message": "..."
  }
}
```

## データモデル概要

### DeviceInfo

- `id`: デバイス識別子
  - Windows: `physicaldrive:<n>`
  - Linux NVMe: `nvme:<controller>`
  - Linux ATA: `ata:<block>`
- `path`: 例 `/dev/sda`, `/dev/nvme0`, `\\\\.\\PhysicalDrive0`
- `protocol`: `ata` または `nvme`
- `model`, `serial`, `firmware`, `capacity_bytes`

### SmartReport

- `device`: `DeviceInfo`
- `collected_at_utc`: RFC3339 UTC タイムスタンプ
- `summary`: 温度、稼働時間、使用率などの要約
- `raw`: ATA/NVMe の生データ構造

## エラーコード

- `invalid_argument`
- `permission_denied`
- `not_found`
- `unsupported_device`
- `unsupported_platform`
- `io_error`
- `internal_error`

## テスト

通常テスト:

```bash
cargo test
```

実機を使う統合テスト (`tests/integration.rs`) は `ignore` されています。
必要に応じて環境変数を指定して実行してください。

```bash
# 例
# Windows: set GETSMART_TEST_DEVICE_ID=physicaldrive:0
# Linux:   export GETSMART_TEST_DEVICE_ID=nvme:nvme0
cargo test -- --ignored
```

## 注意事項

- 本プロジェクトは SMART 取得のために低レベル I/O を使用します。
- デバイスや環境によっては一部項目が `null` になります。
- USB 接続デバイスや仮想デバイスは対象外になる場合があります。

## ライセンス

必要に応じて追記してください。

//! 加密层：master key 管理 + AES-256-GCM encrypt/decrypt。
//!
//! 落盘格式：`base64( nonce(12 bytes) || ciphertext )`，每次加密使用独立随机 nonce。

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rand::RngCore;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::paths;

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;

/// 以 0600 权限写文件，存在则覆盖。
///
/// 先写入再设权限：对新文件而言文件系统默认的 umask 可能允许组/其他可读，因此显式 `0o600`。
pub fn write_secure(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建父目录失败: {}", parent.display()))?;
    }
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("打开文件失败: {}", path.display()))?;
    f.write_all(contents)
        .with_context(|| format!("写入文件失败: {}", path.display()))?;
    f.sync_all().ok();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("设置文件权限失败: {}", path.display()))?;
    Ok(())
}

/// 读取 `.master_key`；不存在则生成 32 字节随机根密钥并以 0600 落盘。
pub fn load_or_create_master_key() -> Result<[u8; KEY_LEN]> {
    let path = paths::master_key_path()?;
    if path.exists() {
        let data = fs::read(&path)
            .with_context(|| format!("读取 master key 失败: {}", path.display()))?;
        if data.len() != KEY_LEN {
            return Err(anyhow!(
                "master key 长度异常：期望 {KEY_LEN} 字节，实际 {}",
                data.len()
            ));
        }
        let mut key = [0u8; KEY_LEN];
        key.copy_from_slice(&data);
        return Ok(key);
    }

    let mut key = [0u8; KEY_LEN];
    OsRng.fill_bytes(&mut key);
    write_secure(&path, &key)?;
    Ok(key)
}

/// AES-256-GCM 加密：随机 nonce，输出 `base64(nonce || ciphertext)`。
pub fn encrypt(plaintext: &str, key: &[u8; KEY_LEN]) -> Result<String> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| anyhow!("AES-GCM 加密失败: {e}"))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(B64.encode(out))
}

/// 解密 `base64(nonce || ciphertext)` 回明文 String。
pub fn decrypt(b64: &str, key: &[u8; KEY_LEN]) -> Result<String> {
    let raw = B64.decode(b64.as_bytes()).context("base64 解码失败")?;
    if raw.len() <= NONCE_LEN {
        return Err(anyhow!("密文长度异常：缺少 nonce"));
    }
    let (nonce_bytes, ct) = raw.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);
    let pt = cipher
        .decrypt(nonce, ct)
        .map_err(|e| anyhow!("AES-GCM 解密失败: {e}"))?;
    String::from_utf8(pt).context("解密结果不是合法 UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_ascii() {
        let mut key = [0u8; KEY_LEN];
        OsRng.fill_bytes(&mut key);
        let plain = "sk-ant-api03-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        let ct = encrypt(plain, &key).unwrap();
        let pt = decrypt(&ct, &key).unwrap();
        assert_eq!(pt, plain);
    }

    #[test]
    fn round_trip_unicode() {
        let mut key = [0u8; KEY_LEN];
        OsRng.fill_bytes(&mut key);
        let plain = "密钥-🔐-test";
        let ct = encrypt(plain, &key).unwrap();
        assert_eq!(decrypt(&ct, &key).unwrap(), plain);
    }

    #[test]
    fn different_nonce_each_call() {
        let mut key = [0u8; KEY_LEN];
        OsRng.fill_bytes(&mut key);
        let a = encrypt("same", &key).unwrap();
        let b = encrypt("same", &key).unwrap();
        // 同明文同 key，因 nonce 随机，密文应不同
        assert_ne!(a, b);
    }

    #[test]
    fn wrong_key_fails() {
        let mut k1 = [0u8; KEY_LEN];
        let mut k2 = [0u8; KEY_LEN];
        OsRng.fill_bytes(&mut k1);
        OsRng.fill_bytes(&mut k2);
        let ct = encrypt("payload", &k1).unwrap();
        assert!(decrypt(&ct, &k2).is_err());
    }
}

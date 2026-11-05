use base64::{engine::general_purpose::STANDARD, Engine};
use minisign_verify::{PublicKey, Signature};

pub(super) fn verify(bytes: &[u8], signature: &str, key: &str) -> Result<(), String> {
    let decode = |value: &str| -> Result<String, String> {
        String::from_utf8(STANDARD.decode(value.trim()).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    };
    let key = PublicKey::decode(&decode(key)?).map_err(|e| e.to_string())?;
    let signature = Signature::decode(&decode(signature)?).map_err(|e| e.to_string())?;
    key.verify(bytes, &signature, true)
        .map_err(|e| e.to_string())?;

    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    const KEY: &str = include_str!("testdata/public-key.txt");
    const PAYLOAD: &[u8] = include_bytes!("testdata/payload.bin");
    const SIGNATURE: &str = include_str!("testdata/payload.bin.sig");
    #[test]
    fn accepts_tauri_cli_signatures_and_rejects_tampering() {
        verify(PAYLOAD, SIGNATURE, KEY).unwrap();
        let mut modified = PAYLOAD.to_vec();
        modified.push(0);
        assert!(verify(&modified, SIGNATURE.trim(), KEY.trim()).is_err());
        let mut signature = STANDARD.decode(SIGNATURE.trim()).unwrap();
        let index = signature.iter().position(|b| *b == b'\n').unwrap() + 10;
        signature[index] = if signature[index] == b'A' { b'B' } else { b'A' };
        assert!(verify(PAYLOAD, &STANDARD.encode(signature), KEY.trim()).is_err());
        let mut key = STANDARD.decode(KEY.trim()).unwrap();
        let last = key.len() - 4;
        key[last] = if key[last] == b'A' { b'B' } else { b'A' };
        assert!(verify(PAYLOAD, SIGNATURE.trim(), &STANDARD.encode(key)).is_err());
    }
    #[test]
    fn unsigned_and_malformed_inputs_are_rejected() {
        assert!(verify(b"payload", "", "").is_err());
        assert!(verify(b"payload", "not-base64", "not-base64").is_err());
    }
}

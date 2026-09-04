use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

#[must_use]
pub fn quote(value: &OsStr) -> String {
    format!("'{}'", escape_bytes(value.as_bytes()))
}

#[must_use]
pub fn escape_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len());
    for &byte in bytes {
        match byte {
            b'\n' => output.push_str("\\n"),
            b'\r' => output.push_str("\\r"),
            b'\t' => output.push_str("\\t"),
            b'\\' => output.push_str("\\\\"),
            b'\'' => output.push_str("'\\''"),
            b' '..=b'~' => output.push(char::from(byte)),
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                output.push_str("\\x");
                output.push(char::from(HEX[usize::from(byte >> 4)]));
                output.push(char::from(HEX[usize::from(byte & 0x0f)]));
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn quoting_is_byte_preserving_and_terminal_safe() {
        assert_eq!(quote(OsStr::new("a b")), "'a b'");
        assert_eq!(
            quote(OsStr::from_bytes(b"a'\\\n\x1b\xff")),
            "'a'\\''\\\\\\n\\x1B\\xFF'"
        );
    }
}

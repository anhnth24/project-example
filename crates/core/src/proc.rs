//! Tiện ích spawn tiến trình con.

use std::ffi::OsStr;
use std::process::Command;

/// Tạo `Command` không mở cửa sổ console trên Windows.
///
/// App desktop (GUI, không có console) spawn tesseract/python/CLI: Windows sẽ
/// cấp console mới cho tiến trình con và cửa sổ đen nháy lên mỗi lần convert.
/// `CREATE_NO_WINDOW` chặn việc đó; stdout/stderr vẫn capture qua pipe như thường.
pub(crate) fn background_command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[cfg(test)]
mod tests {
    // CREATE_NO_WINDOW chỉ được ẩn cửa sổ, không được phá spawn/capture.
    #[cfg(windows)]
    #[test]
    fn background_command_still_spawns_and_captures_output() {
        let output = super::background_command("cmd")
            .args(["/C", "echo hi"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("hi"));
    }
}

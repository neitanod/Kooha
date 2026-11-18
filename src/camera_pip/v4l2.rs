//! Reads and writes V4L2 camera controls with `VIDIOC_G_CTRL` and
//! `VIDIOC_S_CTRL`, so that Kooha does not depend on `v4l-utils`.

use std::{
    fs::{File, OpenOptions},
    io,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::Path,
};

/// `V4L2_CID_EXPOSURE_AUTO_PRIORITY`, which `v4l2-ctl` lists as
/// `exposure_dynamic_framerate`: with 1, the camera may lower its frame rate
/// to expose longer in low light.
pub const EXPOSURE_DYNAMIC_FRAMERATE: u32 = 0x009a_0903;

/// `_IOWR('V', 27, struct v4l2_control)`.
const VIDIOC_G_CTRL: libc::Ioctl = 0xc008_561b;
/// `_IOWR('V', 28, struct v4l2_control)`.
const VIDIOC_S_CTRL: libc::Ioctl = 0xc008_561c;

/// `struct v4l2_control` of `linux/videodev2.h`.
#[repr(C)]
struct V4l2Control {
    id: u32,
    value: i32,
}

/// The value of control `id` of the device node at `path`.
pub fn get_control(path: &Path, id: u32) -> io::Result<i32> {
    let mut control = V4l2Control { id, value: 0 };
    control_ioctl(&open(path)?, VIDIOC_G_CTRL, &mut control)?;
    Ok(control.value)
}

/// Sets control `id` of the device node at `path` to `value`.
pub fn set_control(path: &Path, id: u32, value: i32) -> io::Result<()> {
    let mut control = V4l2Control { id, value };
    control_ioctl(&open(path)?, VIDIOC_S_CTRL, &mut control)
}

/// The device node of a camera path, if it is a V4L2 node. Other paths,
/// such as the display name a device without a path gets as its id, are
/// never opened.
pub fn video_node(path: &str) -> Option<&Path> {
    let number = path.strip_prefix("/dev/video")?;
    (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| Path::new(path))
}

/// Opens the node without waiting for a frame, as V4L2 tools do.
fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

fn control_ioctl(file: &File, request: libc::Ioctl, control: &mut V4l2Control) -> io::Result<()> {
    // SAFETY: `file` is an open descriptor and `control` is a valid
    // `struct v4l2_control`, the argument both requests take.
    let result = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            request,
            std::ptr::from_mut(control).cast::<libc::c_void>(),
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `_IOWR(type, nr, size)` of `asm-generic/ioctl.h`.
    fn iowr(kind: u8, nr: u8, size: usize) -> libc::Ioctl {
        let size = libc::Ioctl::try_from(size).unwrap();
        (3 << 30) | (size << 16) | (libc::Ioctl::from(kind) << 8) | libc::Ioctl::from(nr)
    }

    #[test]
    fn ioctl_numbers_match_linux_headers() {
        assert_eq!(std::mem::size_of::<V4l2Control>(), 8);
        assert_eq!(VIDIOC_G_CTRL, 0xc008_561b);
        assert_eq!(VIDIOC_S_CTRL, 0xc008_561c);
        assert_eq!(VIDIOC_G_CTRL, iowr(b'V', 27, 8));
        assert_eq!(VIDIOC_S_CTRL, iowr(b'V', 28, 8));
    }

    /// `/dev/null` opens but knows no V4L2 ioctl. The tests never open
    /// `/dev/video*`: that would turn on the camera light.
    #[test]
    fn control_on_a_non_camera_is_an_error() {
        let null = Path::new("/dev/null");
        assert!(get_control(null, EXPOSURE_DYNAMIC_FRAMERATE).is_err());
        assert!(set_control(null, EXPOSURE_DYNAMIC_FRAMERATE, 0).is_err());
    }

    #[test]
    fn only_video_nodes_are_opened() {
        assert_eq!(video_node("/dev/video0"), Some(Path::new("/dev/video0")));
        assert_eq!(video_node("/dev/video12"), Some(Path::new("/dev/video12")));
        assert_eq!(video_node(""), None);
        assert_eq!(video_node("/dev/null"), None);
        assert_eq!(video_node("/dev/video"), None);
        assert_eq!(video_node("/dev/video0/../sda"), None);
        assert_eq!(video_node("Integrated Camera"), None);
    }

    #[test]
    fn control_on_a_missing_node_is_an_error() {
        let missing = Path::new("/nonexistent/kooha-video");
        let err = get_control(missing, EXPOSURE_DYNAMIC_FRAMERATE).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}

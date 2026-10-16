use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

use byteorder::{BigEndian, ReadBytesExt};
use eyre::{WrapErr, bail, eyre};
use tracing::instrument;

use crate::{
    danmaku_xml_to_ass::{DamakuXmlDTag, DanmakuXmlITag},
    protobuf::DmSegMobileReply,
    runtime::AppContext,
};

pub fn filename_filter(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\\' | '/' | '\n' => ' ',
            ':' => '：',
            '*' => '⭐',
            '?' => '？',
            '"' => '\'',
            '<' => '《',
            '>' => '》',
            '|' => '丨',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .trim_end_matches('.')
        .trim()
        .to_string()
}

enum BoxSizeField {
    SizeExtendToEnd,
    LargeSize,
    NormalSize(u32),
}

impl From<u32> for BoxSizeField {
    fn from(size: u32) -> Self {
        match size {
            0 => BoxSizeField::SizeExtendToEnd,
            1 => BoxSizeField::LargeSize,
            _ => BoxSizeField::NormalSize(size),
        }
    }
}

#[instrument(level = "error", skip_all, fields(file_path = ?file_path))]
pub fn is_mp4_complete(file_path: &Path) -> eyre::Result<bool> {
    let file = File::open(file_path).wrap_err(format!("打开文件`{}`失败", file_path.display()))?;
    let real_size = file
        .metadata()
        .wrap_err(format!("获取文件`{}`元数据失败", file_path.display()))?
        .len();
    let mut reader = BufReader::new(file);
    let mut total_size: u64 = 0;

    let mut has_moov_box = false;
    let mut is_first_box = true;

    loop {
        // 读取Box尺寸字段
        let box_size_field: BoxSizeField = match reader.read_u32::<BigEndian>() {
            Ok(s) => s.into(),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break, // 正常结束
            Err(e) => return Err(eyre!(e)),
        };
        // 读取Box类型字段
        let mut box_type_bytes = [0u8; 4];
        if let Err(e) = reader.read_exact(&mut box_type_bytes) {
            // 如果在读取type时就结束了，说明文件在box header中间被截断
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok(false);
            }
            return Err(eyre!(e));
        }
        // 如果是第一个Box，检查是否是 'ftyp' Box
        if is_first_box {
            if &box_type_bytes != b"ftyp" {
                // 如果第一个box不是 'ftyp'，则认为它不是一个标准的MP4文件
                return Ok(false);
            }
            is_first_box = false;
        }
        // 检查是否有 'moov' Box
        if &box_type_bytes == b"moov" {
            has_moov_box = true;
        }

        // 获取Box尺寸
        let box_size = match box_size_field {
            // Box延伸到文件末尾，直接返回true
            BoxSizeField::SizeExtendToEnd => return Ok(true),
            BoxSizeField::LargeSize => {
                // 对于尺寸非常大的Box(大于4GB)，其真实的尺寸是一个64位的整数，紧跟在类型字段后面
                let large_box_size = reader.read_u64::<BigEndian>()?;
                // 头部总共16字节(box_size 4 + box_type 4 + large_box_size 8)
                if large_box_size < 16 {
                    // 如果连16字节都不够，说明有问题
                    return Ok(false);
                }
                // 跳过Box剩余的部分
                #[allow(clippy::cast_possible_wrap)]
                reader.seek_relative((large_box_size - 16) as i64)?;
                large_box_size
            }
            BoxSizeField::NormalSize(box_size) => {
                // 头部总共8字节 (size 4 + type 4)
                if box_size < 8 {
                    // 如果连8字节都不够，说明有问题
                    return Ok(false);
                }
                // 跳过Box剩余的部分
                reader.seek_relative(i64::from(box_size - 8))?;

                u64::from(box_size)
            }
        };

        total_size += box_size;

        if total_size > real_size {
            // 如果总大小超过了实际文件大小，说明有问题
            return Ok(false);
        }
    }

    Ok(real_size == total_size && has_moov_box)
}

pub trait ToXml {
    fn to_xml(&self, cid: i64) -> eyre::Result<String>;
}

impl ToXml for Vec<DmSegMobileReply> {
    #[instrument(level = "error", skip_all, fields(cid = cid))]
    fn to_xml(&self, cid: i64) -> eyre::Result<String> {
        let elems = self
            .iter()
            .flat_map(|reply| &reply.elems)
            .map(|elem| DamakuXmlDTag {
                p: format!(
                    "{},{},{},{},{},{},{},{}",
                    elem.progress / 1000,
                    elem.mode,
                    elem.fontsize,
                    elem.color,
                    elem.ctime,
                    elem.pool,
                    elem.mid_hash.clone(),
                    elem.id_str.clone(),
                ),
                body: Some(elem.content.clone()),
            })
            .collect();

        let i_tag = DanmakuXmlITag { chatid: cid, elems };

        let xml = yaserde::ser::to_string(&i_tag).map_err(|e| eyre!(e))?;

        Ok(xml)
    }
}

#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cast_sign_loss)]
#[allow(clippy::similar_names)]
pub fn seconds_to_srt_time(seconds: f64) -> String {
    let total_ms = (seconds * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let total_m = total_s / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    format!("{h:02}:{m:02}:{s:02},{ms:03}")
}

#[instrument(level = "error", skip_all)]
pub fn get_ffmpeg_program(app: &AppContext) -> eyre::Result<PathBuf> {
    let configured = app.get_config().read().ffmpeg_path.clone();
    let executable_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    resolve_ffmpeg_program(configured.as_deref(), executable_dir.as_deref())
}

fn resolve_ffmpeg_program(
    configured: Option<&Path>,
    executable_dir: Option<&Path>,
) -> eyre::Result<PathBuf> {
    if let Some(path) = configured {
        check_executable(path).wrap_err_with(|| {
            format!(
                "指定的 FFmpeg 无法执行：{}；请用 --ffmpeg 指定存在且可执行的 FFmpeg 文件",
                path.display()
            )
        })?;
        return std::fs::canonicalize(path).wrap_err("解析指定的 FFmpeg 文件路径失败");
    }
    if let Some(dir) = executable_dir {
        let name = if cfg!(target_os = "windows") {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        };
        let sibling = dir.join(name);
        if check_executable(&sibling).is_ok() {
            return Ok(sibling);
        }
    }
    // Command resolves this through PATH. Its spawn error includes the install
    // and --ffmpeg instructions, so callers do not need to mutate PATH.
    Ok(PathBuf::from("ffmpeg"))
}

fn check_executable(path: &Path) -> eyre::Result<()> {
    let metadata = std::fs::metadata(path).wrap_err("可执行文件不存在或无法访问")?;
    if !metadata.is_file() {
        bail!("路径不是文件");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            bail!("文件没有执行权限");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_executable(path: &Path) {
        std::fs::write(path, b"test executable").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn ffmpeg_selection_prefers_explicit_then_sibling_then_path() {
        let temp_dir =
            std::env::temp_dir().join(format!("bili-ffmpeg-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        assert_eq!(
            resolve_ffmpeg_program(None, Some(&temp_dir)).unwrap(),
            PathBuf::from("ffmpeg")
        );
        let sibling = temp_dir.join(if cfg!(target_os = "windows") {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        });
        make_executable(&sibling);
        assert_eq!(
            resolve_ffmpeg_program(None, Some(&temp_dir)).unwrap(),
            sibling
        );
        let explicit = temp_dir.join("custom-ffmpeg");
        make_executable(&explicit);
        assert_eq!(
            resolve_ffmpeg_program(Some(&explicit), Some(&temp_dir)).unwrap(),
            std::fs::canonicalize(explicit).unwrap()
        );
        std::fs::remove_dir_all(temp_dir).unwrap();
    }

    #[test]
    fn ffmpeg_explicit_missing_or_directory_is_an_error_without_fallback() {
        let temp_dir =
            std::env::temp_dir().join(format!("bili-ffmpeg-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let missing = temp_dir.join("missing-ffmpeg");
        let error = resolve_ffmpeg_program(Some(&missing), Some(&temp_dir)).unwrap_err();
        assert!(error.to_string().contains("--ffmpeg"));
        assert!(resolve_ffmpeg_program(Some(&temp_dir), None).is_err());
        std::fs::remove_dir_all(temp_dir).unwrap();
    }

    #[test]
    fn ffmpeg_explicit_relative_file_is_resolved_before_execution() {
        let name = format!("bili-ffmpeg-test-{}", uuid::Uuid::new_v4());
        let relative = Path::new(&name);
        make_executable(relative);
        let result = resolve_ffmpeg_program(Some(relative), None).unwrap();
        assert!(result.is_absolute());
        assert_eq!(result, std::fs::canonicalize(relative).unwrap());
        std::fs::remove_file(relative).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_explicit_non_executable_file_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let file = std::env::temp_dir().join(format!("bili-ffmpeg-test-{}", uuid::Uuid::new_v4()));
        std::fs::write(&file, b"not executable").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(resolve_ffmpeg_program(Some(&file), None).is_err());
        std::fs::remove_file(file).unwrap();
    }
}

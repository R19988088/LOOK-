//! 档案层：把"文件夹 / zip / 7z"抽象成统一的页序列，并按页随机读取。

use std::cmp::Ordering;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use zip::ZipArchive;

pub const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "jfif", "png", "webp", "gif", "bmp", "tif", "tiff", "avif", "tga", "jp2", "qoi",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BookKind {
    Dir,
    Zip,
    SevenZ,
}

#[derive(Clone, Debug)]
pub struct Book {
    pub path: PathBuf,
    pub title: String,
    pub kind: BookKind,
    /// zip/7z 里是条目名；文件夹里是绝对路径
    pub pages: Vec<String>,
}

impl Book {
    pub fn open(path: &Path) -> Result<Self> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let title = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("untitled")
            .to_string();

        // 单张图片：同目录里的其他图片一起排成序列，所以打开单图也能左右翻
        if path.is_file() && IMAGE_EXTS.contains(&ext.as_str()) {
            let mut pages = Vec::new();
            if let Some(dir) = path.parent() {
                collect_images_shallow(dir, &mut pages);
                pages.sort_by(|a, b| natural_cmp(a, b));
            }
            if pages.len() <= 1 {
                pages = vec![path.to_string_lossy().to_string()];
            }
            return Ok(Self {
                path: path.to_path_buf(),
                title,
                kind: BookKind::Dir,
                pages,
            });
        }

        let mut kind = match ext.as_str() {
            "zip" | "cbz" | "cbr" => BookKind::Zip,
            "7z" | "cb7" => BookKind::SevenZ,
            _ => {
                if path.is_dir() {
                    BookKind::Dir
                } else {
                    bail!("不支持的格式: .{ext}")
                }
            }
        };

        let pages = match kind {
            BookKind::Dir => {
                let mut v = Vec::new();
                collect_dir(path, &mut v)?;
                v.sort_by(|a, b| natural_cmp(a, b));
                v
            }
            BookKind::Zip => {
                let f = fs::File::open(path).context("打开压缩包失败")?;
                let mut za = ZipArchive::new(f).context("解析 zip 中央目录失败")?;
                let mut v = Vec::new();
                for i in 0..za.len() {
                    if let Ok(entry) = za.by_index(i) {
                        let name = entry.name().map(|n| n.to_string()).unwrap_or_default();
                        if is_image_name(&name) && !entry.is_dir() {
                            v.push(name);
                        }
                    }
                }
                v.sort_by(|a, b| natural_cmp(a, b));
                v
            }
            BookKind::SevenZ => {
                // 7z 默认是固体压缩：随机读单页会把前面所有数据重复解压一遍，上千页必然卡死。
                // 因此首次打开时整包解压到本地缓存（目录即"书源"），之后随机读都是本地文件。
                let arch = sevenz_rust2::Archive::open(path).context("解析 7z 失败")?;
                let mut names = Vec::new();
                for f in &arch.files {
                    if f.has_stream() && is_image_name(f.name()) {
                        names.push(f.name().to_string());
                    }
                }
                if names.is_empty() {
                    bail!("这里没有找到图片");
                }
                names.sort_by(|a, b| natural_cmp(a, b));

                let dir = sevenz_cache_dir(path)?;
                if !dir.join(".done").exists() {
                    let _ = fs::remove_dir_all(&dir);
                    fs::create_dir_all(&dir).context("创建 7z 缓存目录失败")?;
                    sevenz_rust2::decompress_file(path, &dir).context("解压 7z 失败")?;
                    let _ = fs::write(dir.join(".done"), b"ok");
                }
                let mut v = Vec::new();
                for n in &names {
                    let p = dir.join(n.replace('/', std::path::MAIN_SEPARATOR_STR));
                    if p.exists() {
                        v.push(p.to_string_lossy().to_string());
                    }
                }
                if v.is_empty() {
                    bail!("7z 解压后没有找到图片");
                }
                v.sort_by(|a, b| natural_cmp(a, b));
                kind = BookKind::Dir;
                v
            }
        };

        if pages.is_empty() {
            bail!("这里没有找到图片");
        }

        Ok(Self {
            path: path.to_path_buf(),
            title,
            kind,
            pages,
        })
    }

    pub fn len(&self) -> usize {
        self.pages.len()
    }

    pub fn page(&self, i: usize) -> Option<&str> {
        self.pages.get(i).map(|s| s.as_str())
    }
}

fn collect_images_shallow(dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            continue;
        }
        if p.extension()
            .and_then(|e| e.to_str())
            .map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
            .unwrap_or(false)
        {
            out.push(p.to_string_lossy().to_string());
        }
    }
}

fn collect_dir(dir: &Path, out: &mut Vec<String>) -> Result<()> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).with_context(|| format!("读取目录 {}", d.display()))? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
                .unwrap_or(false)
            {
                out.push(p.to_string_lossy().to_string());
            }
        }
    }
    Ok(())
}

/// 7z 解压缓存的目录：按 路径+大小+修改时间 做 FNV 哈希，内容变了自然换目录。
fn sevenz_cache_dir(path: &Path) -> Result<PathBuf> {
    let meta = fs::metadata(path).context("读取 7z 文件信息失败")?;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    mix(path.to_string_lossy().as_bytes());
    mix(&meta.len().to_le_bytes());
    if let Ok(t) = meta.modified() {
        if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
            mix(&d.as_secs().to_le_bytes());
        }
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("comicpp")
        .join("sz");
    Ok(base.join(format!("{h:016x}")))
}

fn is_image_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// 自然排序：`1.jpg 2.jpg ... 10.jpg` 而不是 `1, 10, 2`。
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.bytes().peekable(), b.bytes().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                if x.is_ascii_digit() && y.is_ascii_digit() {
                    let mut na = 0u64;
                    while let Some(c) = ai.peek().copied() {
                        if !c.is_ascii_digit() {
                            break;
                        }
                        na = na.saturating_mul(10).saturating_add((c - b'0') as u64);
                        ai.next();
                    }
                    let mut nb = 0u64;
                    while let Some(c) = bi.peek().copied() {
                        if !c.is_ascii_digit() {
                            break;
                        }
                        nb = nb.saturating_mul(10).saturating_add((c - b'0') as u64);
                        bi.next();
                    }
                    match na.cmp(&nb) {
                        Ordering::Equal => {}
                        o => return o,
                    }
                } else {
                    let xl = x.to_ascii_lowercase();
                    let yl = y.to_ascii_lowercase();
                    match xl.cmp(&yl) {
                        Ordering::Equal => {
                            ai.next();
                            bi.next();
                        }
                        o => return o,
                    }
                }
            }
        }
    }
}

/// 每页的原始字节（已解压，尚未 decode）。
#[derive(Clone, Debug)]
pub struct RawPage {
    pub bytes: Vec<u8>,
    pub name: String,
}

/// 读取器：每个解码线程各持一个，避免反复解析中央目录。
pub struct Reader {
    book: Book,
    zip: Option<ZipArchive<fs::File>>,
}

impl Reader {
    pub fn open(book: &Book) -> Result<Self> {
        let zip = match book.kind {
            BookKind::Zip => Some(
                ZipArchive::new(fs::File::open(&book.path).context("打开压缩包失败")?)
                    .context("解析 zip 中央目录失败")?,
            ),
            _ => None,
        };
        Ok(Self {
            book: book.clone(),
            zip,
        })
    }

    pub fn read(&mut self, index: usize) -> Result<RawPage> {
        let name = self
            .book
            .pages
            .get(index)
            .cloned()
            .ok_or_else(|| anyhow!("页码越界: {index}"))?;

        match self.book.kind {
            BookKind::Dir => {
                let bytes = fs::read(&name).with_context(|| format!("读取 {}", name))?;
                Ok(RawPage { bytes, name })
            }
            BookKind::Zip => {
                let za = self.zip.as_mut().ok_or_else(|| anyhow!("压缩包未打开"))?;
                let mut entry = za
                    .by_name(&name)
                    .with_context(|| format!("包内缺少 {name}"))?;
                let mut bytes = Vec::with_capacity(entry.size() as usize);
                entry.read_to_end(&mut bytes)?;
                Ok(RawPage { bytes, name })
            }
            BookKind::SevenZ => bail!("7z 暂未启用"),
        }
    }
}

/// 只用解码器判断尺寸，不为缩略图分配大缓冲的便宜路径留给后续优化。
pub fn decode_bytes(bytes: &[u8]) -> Result<image::RgbaImage> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("无法识别图片格式")?;
    let img = reader.decode().context("解码失败")?;
    Ok(img.to_rgba8())
}

//! Fusion-Design 素材库管理 — 参考图上传/分类/标注/色彩提取/设计系统绑定。
//!
//! 对应 PRD §3.2.3「设计系统+素材库」。提供素材的增删改查、分类标注、
//! 色彩提取、设计系统 Token 绑定等核心能力。
//!
//! 【离线硬约束】所有操作走本地文件系统，无公网调用。
//!
//! Callers: fd-cli (asset subcommands), fd-host-web (asset panel), fd-ai-adapter (ImageToUiSkill reference)
//! Affected API: AssetLibrary, AssetItem, AssetCategory, ColorExtraction, Annotation
//! Data schemas: AssetItem (image metadata + annotations + color palette), AssetLibrary (collection + search)
//! User instruction: "按照你建议的优先级马上启动落地" — P0 素材库管理模块

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ── 安全上限（防图像解码 DoS）──
const MAX_PIXELS: u64 = 64_000_000;
const MAX_IMAGE_FILE_SIZE: u64 = 100 * 1024 * 1024;

// ── 素材条目 ──

/// 素材类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetKind {
    /// 参考图（截图、设计稿、照片等）。
    Image,
    /// 图标素材。
    Icon,
    /// 插画素材。
    Illustration,
    /// 其他。
    Other,
}

/// 素材标注（画在参考图上的标记）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// 色彩提取结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorExtraction {
    /// 提取的主色列表（HEX 格式）。
    pub dominant_colors: Vec<String>,
    /// 每种色的权重（0.0–1.0，与 dominant_colors 一一对应）。
    pub weights: Vec<f32>,
    /// 是否已绑定到设计系统 Token。
    pub bound_tokens: HashMap<String, String>,
}

/// 素材条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetItem {
    pub id: String,
    pub name: String,
    pub kind: AssetKind,
    /// 本地文件路径（绝对路径）。
    pub file_path: PathBuf,
    /// 所属分类 ID 列表。
    pub categories: Vec<String>,
    /// 标签。
    pub tags: Vec<String>,
    /// 画在素材上的标注。
    #[serde(default)]
    pub annotations: Vec<Annotation>,
    /// 色彩提取结果。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_extraction: Option<ColorExtraction>,
    /// 绑定的设计系统 ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design_system_id: Option<String>,
    /// 文件大小（字节）。
    #[serde(default)]
    pub file_size: u64,
    /// 图片宽度（像素，仅 Image/Icon/lllustration）。
    #[serde(default)]
    pub width: u32,
    /// 图片高度。
    #[serde(default)]
    pub height: u32,
    /// 创建时间（Unix timestamp）。
    #[serde(default = "default_timestamp")]
    pub created_at: u64,
    /// 更新时间。
    #[serde(default = "default_timestamp")]
    pub updated_at: u64,
}

fn default_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── 分类 ──

/// 素材分类。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetCategory {
    pub id: String,
    pub name: String,
    /// 父分类 ID（None 表示顶级分类）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// 分类图标标识。
    #[serde(default)]
    pub icon: String,
    /// 分类描述。
    #[serde(default)]
    pub description: String,
}

// ── 素材库 ──

/// 素材库（本地文件持久化）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssetLibrary {
    /// 全部素材条目。
    pub items: HashMap<String, AssetItem>,
    /// 全部分类。
    pub categories: HashMap<String, AssetCategory>,
}

impl AssetLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加素材条目。
    pub fn add(&mut self, item: AssetItem) {
        tracing::info!(id = %item.id, name = %item.name, "add: 素材已添加");
        self.items.insert(item.id.clone(), item);
    }

    /// 按 ID 移除素材。
    pub fn remove(&mut self, id: &str) -> Option<AssetItem> {
        let removed = self.items.remove(id);
        if removed.is_some() {
            tracing::info!(id = %id, "remove: 素材已移除");
        }
        removed
    }

    /// 按 ID 查找素材。
    pub fn get(&self, id: &str) -> Option<&AssetItem> {
        self.items.get(id)
    }

    /// 按 ID 查找素材（可变）。
    pub fn get_mut(&mut self, id: &str) -> Option<&mut AssetItem> {
        self.items.get_mut(id)
    }

    /// 添加分类。
    pub fn add_category(&mut self, cat: AssetCategory) {
        tracing::info!(id = %cat.id, name = %cat.name, "add_category: 分类已添加");
        self.categories.insert(cat.id.clone(), cat);
    }

    /// 按分类 ID 查找素材列表。
    pub fn items_by_category(&self, category_id: &str) -> Vec<&AssetItem> {
        self.items
            .values()
            .filter(|i| i.categories.iter().any(|c| c == category_id))
            .collect()
    }

    /// 按关键词搜索素材（匹配 name/tags/分类名）。
    pub fn search(&self, query: &str) -> Vec<&AssetItem> {
        let q = query.to_lowercase();
        self.items
            .values()
            .filter(|i| {
                if q.is_empty() {
                    return true;
                }
                let name_match = i.name.to_lowercase().contains(&q);
                let tag_match = i.tags.iter().any(|t| t.to_lowercase().contains(&q));
                let cat_match = i.categories.iter().any(|c| {
                    self.categories
                        .get(c)
                        .map(|cat| cat.name.to_lowercase().contains(&q))
                        .unwrap_or(false)
                });
                name_match || tag_match || cat_match
            })
            .collect()
    }

    /// 按 tag 精确匹配检索素材。
    pub fn search_by_tags(&self, tags: &[String]) -> Vec<&AssetItem> {
        let lower_tags: Vec<String> = tags.iter().map(|t| t.to_lowercase()).collect();
        self.items
            .values()
            .filter(|i| {
                lower_tags
                    .iter()
                    .all(|t| i.tags.iter().any(|it| it.to_lowercase() == *t))
            })
            .collect()
    }

    /// 按类型筛选素材。
    pub fn filter_by_kind(&self, kind: AssetKind) -> Vec<&AssetItem> {
        self.items.values().filter(|i| i.kind == kind).collect()
    }

    /// 给素材添加标注。
    pub fn add_annotation(&mut self, item_id: &str, annotation: Annotation) -> anyhow::Result<()> {
        let item = self
            .items
            .get_mut(item_id)
            .ok_or_else(|| AssetError::NotFound(item_id.to_string()))?;
        item.annotations.push(annotation);
        item.updated_at = default_timestamp();
        tracing::info!(item_id = %item_id, "add_annotation: 标注已添加");
        Ok(())
    }

    /// 设置色彩提取结果。
    pub fn set_color_extraction(
        &mut self,
        item_id: &str,
        extraction: ColorExtraction,
    ) -> anyhow::Result<()> {
        let item = self
            .items
            .get_mut(item_id)
            .ok_or_else(|| AssetError::NotFound(item_id.to_string()))?;
        item.color_extraction = Some(extraction);
        item.updated_at = default_timestamp();
        tracing::info!(item_id = %item_id, "set_color_extraction: 色彩提取已设置");
        Ok(())
    }

    /// 绑定素材到设计系统。
    pub fn bind_design_system(
        &mut self,
        item_id: &str,
        design_system_id: &str,
    ) -> anyhow::Result<()> {
        let item = self
            .items
            .get_mut(item_id)
            .ok_or_else(|| AssetError::NotFound(item_id.to_string()))?;
        item.design_system_id = Some(design_system_id.to_string());
        item.updated_at = default_timestamp();
        tracing::info!(item_id = %item_id, ds = %design_system_id, "bind_design_system: 已绑定");
        Ok(())
    }

    /// 将色彩提取结果绑定为设计系统 Token。
    pub fn bind_colors_to_tokens(
        &mut self,
        item_id: &str,
        token_map: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        let item = self
            .items
            .get_mut(item_id)
            .ok_or_else(|| AssetError::NotFound(item_id.to_string()))?;
        if let Some(ref mut extraction) = item.color_extraction {
            extraction.bound_tokens.extend(token_map.clone());
            tracing::info!(
                item_id = %item_id,
                count = token_map.len(),
                "bind_colors_to_tokens: Token 绑定完成"
            );
        } else {
            return Err(AssetError::NoColorExtraction(item_id.to_string()).into());
        }
        item.updated_at = default_timestamp();
        Ok(())
    }

    /// 序列化为 JSON。
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// 从 JSON 反序列化。
    pub fn from_json(json: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(json)?)
    }

    /// 持久化到文件。
    pub fn save_to_file(&self, path: &Path) -> anyhow::Result<()> {
        let json = self.to_json()?;
        std::fs::write(path, json)?;
        tracing::info!(path = %path.display(), "save_to_file: 素材库已持久化");
        Ok(())
    }

    /// 从文件加载。
    pub fn load_from_file(path: &Path) -> anyhow::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        let lib = Self::from_json(&json)?;
        tracing::info!(
            path = %path.display(),
            items = lib.items.len(),
            "load_from_file: 素材库已加载"
        );
        Ok(lib)
    }

    /// 素材总数。
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

// ── 素材导入 ──

/// 从本地文件导入素材到素材库。
pub fn import_file(
    library: &mut AssetLibrary,
    file_path: &Path,
    name: &str,
    kind: AssetKind,
    categories: Vec<String>,
    tags: Vec<String>,
) -> anyhow::Result<String> {
    if !file_path.exists() {
        return Err(AssetError::FileNotFound(file_path.to_string_lossy().to_string()).into());
    }

    let metadata = std::fs::metadata(file_path)?;
    let file_size = metadata.len();
    if file_size > MAX_IMAGE_FILE_SIZE {
        tracing::warn!(
            path = %file_path.display(),
            size = file_size,
            limit = MAX_IMAGE_FILE_SIZE,
            "import_file: 文件超过 100MB 上限，拒绝导入"
        );
        return Err(AssetError::FileTooLarge(file_size, MAX_IMAGE_FILE_SIZE).into());
    }
    let id = format!("asset-{}", uuid_simple());

    let (width, height) = read_image_dimensions(file_path);

    let item = AssetItem {
        id: id.clone(),
        name: name.to_string(),
        kind,
        file_path: file_path.to_path_buf(),
        categories,
        tags,
        annotations: vec![],
        color_extraction: None,
        design_system_id: None,
        file_size,
        width,
        height,
        created_at: default_timestamp(),
        updated_at: default_timestamp(),
    };

    library.add(item);
    tracing::info!(id = %id, path = %file_path.display(), "import_file: 素材导入完成");
    Ok(id)
}

/// 简单 UUID（时间戳 + 随机后缀，非标准 UUID）。
fn uuid_simple() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}-{}", (ts & 0xFFFFFFFF), (ts >> 32) & 0xFFFF)
}

/// 读取图片尺寸：仅读 header，不解码像素缓冲。
fn read_image_dimensions(path: &Path) -> (u32, u32) {
    let result = (|| -> Result<(u32, u32), anyhow::Error> {
        let (w, h) = image::ImageReader::open(path)?.into_dimensions()?;
        if w as u64 * h as u64 > MAX_PIXELS {
            tracing::warn!(
                path = %path.display(),
                w,
                h,
                pixels = w as u64 * h as u64,
                limit = MAX_PIXELS,
                "read_image_dimensions: 像素数超过上限，拒绝"
            );
            return Err(AssetError::ImageTooLarge(w, h, MAX_PIXELS).into());
        }
        Ok((w, h))
    })();
    match result {
        Ok((w, h)) => {
            tracing::info!(path = %path.display(), w, h, "read_image_dimensions: 读取成功");
            (w, h)
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "read_image_dimensions: 读取失败");
            (0, 0)
        }
    }
}

/// 从图片提取主色：像素采样 + 分桶量化，返回最多 8 种主色及权重。
pub fn extract_colors(file_path: &Path) -> anyhow::Result<ColorExtraction> {
    let img = image::ImageReader::open(file_path)?
        .decode()
        .map_err(|e| anyhow::anyhow!("图片解码失败: {}", e))?;
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    if w == 0 || h == 0 {
        tracing::warn!("extract_colors: 图片尺寸为 0");
        return Ok(ColorExtraction {
            dominant_colors: vec![],
            weights: vec![],
            bound_tokens: HashMap::new(),
        });
    }

    // 采样间隔：最多采样 4096 个像素
    let step = ((w as usize * h as usize) / 4096).max(1);
    let mut buckets: HashMap<(u8, u8, u8), usize> = HashMap::new();
    let raw_pixels = rgb.as_raw();
    let pixel_count = raw_pixels.len() / 3;
    for i in (0..pixel_count).step_by(step) {
        if i % step != 0 {
            continue;
        }
        // 量化到 32 级（每通道 8 档），减少颜色碎片
        let base = i * 3;
        let qr = (raw_pixels[base] / 32) * 32;
        let qg = (raw_pixels[base + 1] / 32) * 32;
        let qb = (raw_pixels[base + 2] / 32) * 32;
        *buckets.entry((qr, qg, qb)).or_insert(0) += 1;
    }

    let total_samples: usize = buckets.values().sum();
    let mut sorted: Vec<_> = buckets.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    let max_colors = 8;
    let mut dominant_colors = Vec::with_capacity(max_colors);
    let mut weights = Vec::with_capacity(max_colors);
    for ((r, g, b), count) in sorted.iter().take(max_colors) {
        let hex = format!("#{:02X}{:02X}{:02X}", r, g, b);
        dominant_colors.push(hex);
        weights.push(*count as f32 / total_samples as f32);
    }

    tracing::info!(
        path = %file_path.display(),
        colors = dominant_colors.len(),
        "extract_colors: 色彩提取完成"
    );

    Ok(ColorExtraction {
        dominant_colors,
        weights,
        bound_tokens: HashMap::new(),
    })
}

// ── 默认分类预设 ──

/// 返回内置默认分类列表。
pub fn default_categories() -> Vec<AssetCategory> {
    vec![
        AssetCategory {
            id: "cat-reference".into(),
            name: "参考图".into(),
            parent_id: None,
            icon: "image".into(),
            description: "设计参考截图、竞品截图、灵感图".into(),
        },
        AssetCategory {
            id: "cat-icon".into(),
            name: "图标".into(),
            parent_id: None,
            icon: "star".into(),
            description: "图标素材库".into(),
        },
        AssetCategory {
            id: "cat-illustration".into(),
            name: "插画".into(),
            parent_id: None,
            icon: "palette".into(),
            description: "插画、装饰素材".into(),
        },
        AssetCategory {
            id: "cat-brand".into(),
            name: "品牌".into(),
            parent_id: None,
            icon: "shield".into(),
            description: "品牌 LOGO、VI 素材".into(),
        },
        AssetCategory {
            id: "cat-photo".into(),
            name: "照片".into(),
            parent_id: None,
            icon: "camera".into(),
            description: "实景照片素材".into(),
        },
    ]
}

/// 创建包含默认分类的空素材库。
pub fn new_library_with_defaults() -> AssetLibrary {
    let mut lib = AssetLibrary::new();
    for cat in default_categories() {
        lib.add_category(cat);
    }
    lib
}

// ── 错误 ──

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("素材 {0} 未找到")]
    NotFound(String),
    #[error("素材 {0} 无色彩提取结果")]
    NoColorExtraction(String),
    #[error("文件不存在: {0}")]
    FileNotFound(String),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("序列化错误: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("文件过大: {0} 字节，超过上限 {1} 字节")]
    FileTooLarge(u64, u64),
    #[error("图片像素数超限: {0}x{1} = {2} 像素")]
    ImageTooLarge(u32, u32, u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_item(id: &str) -> AssetItem {
        AssetItem {
            id: id.to_string(),
            name: format!("测试素材-{id}"),
            kind: AssetKind::Image,
            file_path: PathBuf::from("/tmp/test.png"),
            categories: vec!["cat-reference".into()],
            tags: vec!["test".into()],
            annotations: vec![],
            color_extraction: None,
            design_system_id: None,
            file_size: 1024,
            width: 100,
            height: 200,
            created_at: 1000,
            updated_at: 1000,
        }
    }

    #[test]
    fn add_and_get_item() {
        let mut lib = AssetLibrary::new();
        let item = sample_item("a1");
        lib.add(item);
        assert!(lib.get("a1").is_some());
        assert!(lib.get("nope").is_none());
    }

    #[test]
    fn remove_item() {
        let mut lib = AssetLibrary::new();
        lib.add(sample_item("a1"));
        let removed = lib.remove("a1");
        assert!(removed.is_some());
        assert!(lib.get("a1").is_none());
    }

    #[test]
    fn add_category_and_query() {
        let mut lib = new_library_with_defaults();
        lib.add(sample_item("a1"));
        let items = lib.items_by_category("cat-reference");
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn search_by_name() {
        let mut lib = AssetLibrary::new();
        lib.add(sample_item("a1"));
        let results = lib.search("测试");
        assert_eq!(results.len(), 1);
        let results = lib.search("不存在");
        assert!(results.is_empty());
    }

    #[test]
    fn search_by_tags() {
        let mut lib = AssetLibrary::new();
        lib.add(sample_item("a1"));
        let results = lib.search_by_tags(&["test".into()]);
        assert_eq!(results.len(), 1);
        let results = lib.search_by_tags(&["missing".into()]);
        assert!(results.is_empty());
    }

    #[test]
    fn filter_by_kind() {
        let mut lib = AssetLibrary::new();
        lib.add(sample_item("a1"));
        let imgs = lib.filter_by_kind(AssetKind::Image);
        assert_eq!(imgs.len(), 1);
        let icons = lib.filter_by_kind(AssetKind::Icon);
        assert!(icons.is_empty());
    }

    #[test]
    fn add_annotation() {
        let mut lib = AssetLibrary::new();
        lib.add(sample_item("a1"));
        let ann = Annotation {
            id: "ann1".into(),
            x: 10.0,
            y: 20.0,
            w: 50.0,
            h: 30.0,
            label: "按钮".into(),
            color: Some("#FF0000".into()),
            note: None,
        };
        lib.add_annotation("a1", ann).unwrap();
        assert_eq!(lib.get("a1").unwrap().annotations.len(), 1);
    }

    #[test]
    fn add_annotation_not_found() {
        let mut lib = AssetLibrary::new();
        let ann = Annotation {
            id: "ann1".into(),
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
            label: String::new(),
            color: None,
            note: None,
        };
        assert!(lib.add_annotation("nope", ann).is_err());
    }

    #[test]
    fn set_color_extraction() {
        let mut lib = AssetLibrary::new();
        lib.add(sample_item("a1"));
        let extraction = ColorExtraction {
            dominant_colors: vec!["#FF5733".into(), "#33FF57".into()],
            weights: vec![0.6, 0.4],
            bound_tokens: HashMap::new(),
        };
        lib.set_color_extraction("a1", extraction).unwrap();
        let item = lib.get("a1").unwrap();
        assert_eq!(
            item.color_extraction
                .as_ref()
                .unwrap()
                .dominant_colors
                .len(),
            2
        );
    }

    #[test]
    fn bind_design_system() {
        let mut lib = AssetLibrary::new();
        lib.add(sample_item("a1"));
        lib.bind_design_system("a1", "apple-hig").unwrap();
        assert_eq!(
            lib.get("a1").unwrap().design_system_id.as_deref(),
            Some("apple-hig")
        );
    }

    #[test]
    fn bind_colors_to_tokens() {
        let mut lib = AssetLibrary::new();
        lib.add(sample_item("a1"));
        let extraction = ColorExtraction {
            dominant_colors: vec!["#FF5733".into()],
            weights: vec![1.0],
            bound_tokens: HashMap::new(),
        };
        lib.set_color_extraction("a1", extraction).unwrap();

        let mut token_map = HashMap::new();
        token_map.insert("color.primary".into(), "#FF5733".into());
        lib.bind_colors_to_tokens("a1", &token_map).unwrap();

        let item = lib.get("a1").unwrap();
        assert_eq!(
            item.color_extraction
                .as_ref()
                .unwrap()
                .bound_tokens
                .get("color.primary"),
            Some(&"#FF5733".to_string())
        );
    }

    #[test]
    fn bind_colors_to_tokens_no_extraction() {
        let mut lib = AssetLibrary::new();
        lib.add(sample_item("a1"));
        let token_map = HashMap::new();
        assert!(lib.bind_colors_to_tokens("a1", &token_map).is_err());
    }

    #[test]
    fn json_roundtrip() {
        let mut lib = new_library_with_defaults();
        lib.add(sample_item("a1"));
        let json = lib.to_json().unwrap();
        let lib2 = AssetLibrary::from_json(&json).unwrap();
        assert_eq!(lib2.items.len(), 1);
        assert_eq!(lib2.categories.len(), 5);
    }

    #[test]
    fn save_and_load_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("library.json");
        let mut lib = new_library_with_defaults();
        lib.add(sample_item("a1"));
        lib.save_to_file(&path).unwrap();
        let lib2 = AssetLibrary::load_from_file(&path).unwrap();
        assert_eq!(lib2.items.len(), 1);
    }

    #[test]
    fn default_categories_count() {
        let cats = default_categories();
        assert_eq!(cats.len(), 5);
    }

    #[test]
    fn len_and_empty() {
        let mut lib = AssetLibrary::new();
        assert!(lib.is_empty());
        assert_eq!(lib.len(), 0);
        lib.add(sample_item("a1"));
        assert!(!lib.is_empty());
        assert_eq!(lib.len(), 1);
    }

    #[test]
    fn extract_colors_missing_file() {
        let result = extract_colors(Path::new("/tmp/__fd_asset_nonexistent__.png"));
        assert!(result.is_err());
    }

    #[test]
    fn annotation_serde() {
        let ann = Annotation {
            id: "ann1".into(),
            x: 10.0,
            y: 20.0,
            w: 50.0,
            h: 30.0,
            label: "按钮".into(),
            color: Some("#FF0000".into()),
            note: Some("主按钮".into()),
        };
        let json = serde_json::to_string(&ann).unwrap();
        let ann2: Annotation = serde_json::from_str(&json).unwrap();
        assert_eq!(ann2.id, "ann1");
        assert_eq!(ann2.note.as_deref(), Some("主按钮"));
    }

    #[test]
    fn color_extraction_serde() {
        let ce = ColorExtraction {
            dominant_colors: vec!["#FF5733".into(), "#33FF57".into()],
            weights: vec![0.6, 0.4],
            bound_tokens: {
                let mut m = HashMap::new();
                m.insert("color.primary".into(), "#FF5733".into());
                m
            },
        };
        let json = serde_json::to_string(&ce).unwrap();
        let ce2: ColorExtraction = serde_json::from_str(&json).unwrap();
        assert_eq!(ce2.dominant_colors.len(), 2);
        assert_eq!(ce2.bound_tokens.len(), 1);
    }

    #[test]
    fn extract_colors_from_real_image() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.png");
        // 创建 10x10 纯红色 PNG
        let img = image::RgbImage::from_pixel(10, 10, image::Rgb([255, 0, 0]));
        img.save(&path).unwrap();
        let result = extract_colors(&path).unwrap();
        assert!(!result.dominant_colors.is_empty());
        assert!(!result.weights.is_empty());
        // 主色应接近红色 #E00000（量化后 255/32*32=224）
        assert!(
            result.dominant_colors[0].starts_with("#E0"),
            "got {}",
            result.dominant_colors[0]
        );
    }

    #[test]
    fn read_image_dimensions_from_real_image() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.png");
        let img = image::RgbImage::from_pixel(20, 30, image::Rgb([0, 0, 0]));
        img.save(&path).unwrap();
        let (w, h) = read_image_dimensions(&path);
        assert_eq!(w, 20);
        assert_eq!(h, 30);
    }
}

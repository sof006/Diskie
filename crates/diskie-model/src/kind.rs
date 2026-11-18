//! File-type categories, decided by extension alone (no content sniffing, no shell calls).

pub const KIND_COUNT: usize = 11;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    Video = 0,
    Images,
    Audio,
    Archives,
    DiskImages,
    Apps,
    Documents,
    Code,
    Caches,
    System,
    Other,
}

impl Kind {
    pub const ALL: [Kind; KIND_COUNT] = [
        Kind::Video,
        Kind::Images,
        Kind::Audio,
        Kind::Archives,
        Kind::DiskImages,
        Kind::Apps,
        Kind::Documents,
        Kind::Code,
        Kind::Caches,
        Kind::System,
        Kind::Other,
    ];

    pub fn from_u8(v: u8) -> Kind {
        Kind::ALL[(v as usize).min(KIND_COUNT - 1)]
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Video => "Video",
            Kind::Images => "Images",
            Kind::Audio => "Audio",
            Kind::Archives => "Archives",
            Kind::DiskImages => "Disk images and VMs",
            Kind::Apps => "Apps and games",
            Kind::Documents => "Documents",
            Kind::Code => "Code and build output",
            Kind::Caches => "Caches and temp",
            Kind::System => "System",
            Kind::Other => "Other",
        }
    }

    /// Classifies a file name by its extension.
    pub fn of_name(name: &str) -> Kind {
        let Some(dot) = name.rfind('.') else {
            return Kind::Other;
        };
        let ext = &name[dot + 1..];
        if ext.is_empty() || ext.len() > 10 {
            return Kind::Other;
        }
        let mut buf = [0u8; 10];
        for (b, c) in buf.iter_mut().zip(ext.bytes()) {
            *b = c.to_ascii_lowercase();
        }
        let ext = &buf[..ext.len()];
        match ext {
            b"mp4" | b"mkv" | b"avi" | b"mov" | b"wmv" | b"webm" | b"m4v" | b"mpg" | b"mpeg" | b"ts" | b"m2ts"
            | b"flv" | b"vob" | b"3gp" | b"mts" => Kind::Video,
            b"jpg" | b"jpeg" | b"png" | b"gif" | b"bmp" | b"tif" | b"tiff" | b"webp" | b"heic" | b"heif" | b"raw"
            | b"cr2" | b"cr3" | b"nef" | b"arw" | b"dng" | b"psd" | b"svg" | b"ico" | b"avif" | b"jxl" | b"exr"
            | b"tga" | b"dds" => Kind::Images,
            b"mp3" | b"flac" | b"wav" | b"aac" | b"ogg" | b"opus" | b"m4a" | b"wma" | b"aiff" | b"aif" | b"alac"
            | b"mid" | b"midi" => Kind::Audio,
            b"zip" | b"7z" | b"rar" | b"tar" | b"gz" | b"tgz" | b"bz2" | b"xz" | b"zst" | b"lz4" | b"cab" | b"lzma"
            | b"pak" | b"pck" | b"jar" | b"nupkg" | b"whl" => Kind::Archives,
            b"iso" | b"img" | b"vhd" | b"vhdx" | b"vmdk" | b"vdi" | b"qcow2" | b"wim" | b"esd" | b"avhdx"
            | b"vmem" | b"vmsn" => Kind::DiskImages,
            b"exe" | b"dll" | b"msi" | b"msix" | b"appx" | b"so" | b"dylib" | b"app" | b"apk" | b"ocx" | b"node"
            | b"pyd" | b"bin" => Kind::Apps,
            b"pdf" | b"doc" | b"docx" | b"xls" | b"xlsx" | b"ppt" | b"pptx" | b"odt" | b"ods" | b"odp" | b"rtf"
            | b"txt" | b"md" | b"epub" | b"csv" | b"pst" | b"ost" | b"one" | b"msg" | b"eml" => Kind::Documents,
            b"rs" | b"c" | b"h" | b"cpp" | b"hpp" | b"cc" | b"cs" | b"java" | b"kt" | b"py" | b"js" | b"mjs"
            | b"ts2" | b"tsx" | b"jsx" | b"go" | b"rb" | b"php" | b"swift" | b"lua" | b"sh" | b"ps1" | b"json"
            | b"toml" | b"yaml" | b"yml" | b"xml" | b"html" | b"css" | b"scss" | b"sql" | b"pdb" | b"obj" | b"o"
            | b"a" | b"lib" | b"rlib" | b"rmeta" | b"pch" | b"ipch" | b"class" | b"pyc" | b"wasm" => Kind::Code,
            b"tmp" | b"temp" | b"cache" | b"log" | b"etl" | b"dmp" | b"mdmp" | b"bak" | b"old" | b"crdownload"
            | b"part" | b"partial" | b"evtx" => Kind::Caches,
            b"sys" | b"cat" | b"mui" | b"inf" | b"drv" | b"edb" | b"dat" | b"db" | b"regtrans-ms" | b"blf"
            | b"manifest" | b"mum" | b"msp" | b"pf" | b"ttf" | b"otf" | b"ttc" | b"fon" => Kind::System,
            _ => Kind::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension_case_insensitively() {
        assert_eq!(Kind::of_name("Holiday.MKV"), Kind::Video);
        assert_eq!(Kind::of_name("pagefile.sys"), Kind::System);
        assert_eq!(Kind::of_name("archive.tar.gz"), Kind::Archives);
        assert_eq!(Kind::of_name("README"), Kind::Other);
        assert_eq!(Kind::of_name(".gitignore"), Kind::Other);
        assert_eq!(Kind::of_name("trailing."), Kind::Other);
    }
}

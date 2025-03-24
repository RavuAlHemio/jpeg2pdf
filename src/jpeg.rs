use std::fmt;
use std::io::{self, Read, Write};

use from_to_repr::from_to_other;


#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Block {
    Short { kind: u8 },
    Long { kind: u8, data: Vec<u8> },
}
impl Block {
    pub fn kind(&self) -> u8 {
        match self {
            Self::Short { kind } => *kind,
            Self::Long { kind, .. } => *kind,
        }
    }

    pub fn data(&self) -> &[u8] {
        match self {
            Self::Short { .. } => &[],
            Self::Long { data, .. } => data.as_slice(),
        }
    }

    pub fn is_required(&self) -> bool {
        let kind = self.kind();
        kind < 0xE0 || kind > 0xFE
    }

    pub fn write<W: Write>(&self, mut writer: W) -> Result<(), Error> {
        match self {
            Self::Short { kind } => {
                let buf2 = [0xFF, *kind];
                writer.write_all(&buf2)?;
                Ok(())
            },
            Self::Long { kind, data } => {
                if data.len() > 0xFFFF {
                    return Err(Error::BlockTooLong { max_allowed: 0xFFFF, obtained: data.len() });
                }

                let mut buf4 = [0xFF, *kind, 0x00, 0x00];
                let len_u16: u16 = (data.len() + 2).try_into().unwrap();
                let len_bytes = len_u16.to_be_bytes();
                buf4[2..4].copy_from_slice(&len_bytes);
                writer.write_all(&buf4)?;
                writer.write_all(data)?;
                Ok(())
            },
        }
    }

    pub fn try_read<R: Read>(mut reader: R) -> Result<Self, Error> {
        let mut buf1 = [0u8];
        reader.read_exact(&mut buf1)?;

        if buf1[0] != 0xFF {
            return Err(Error::NotABlock { start_byte: buf1[0] });
        }

        reader.read_exact(&mut buf1)?;
        let block_kind = buf1[0];

        match block_kind {
            0xD0..=0xD9 => {
                // restart 0 through 7, start-of-image, end-of-image, start-of-scan
                // short blocks
                Ok(Block::Short { kind: block_kind })
            },
            _ => {
                // long blocks
                let mut buf2 = [0u8; 2];
                reader.read_exact(&mut buf2)?;
                let block_len_incl_len: usize = u16::from_be_bytes(buf2).into();
                if block_len_incl_len < 2 {
                    return Err(Error::BlockTooShort { min_expected: 2, obtained: block_len_incl_len });
                }
                let block_len = block_len_incl_len - 2;

                let mut data = vec![0u8; block_len];
                reader.read_exact(&mut data)?;
                Ok(Block::Long { kind: block_kind, data })
            },
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    NotABlock { start_byte: u8 },
    BlockTooShort { min_expected: usize, obtained: usize },
    BlockTooLong { max_allowed: usize, obtained: usize },
    IncompleteData { builder: ImageBuilder },
    UnexpectedBlock { expected_kind: u8, obtained_kind: u8 },
    IncorrectImageDataTermination,
    NotJfif,
    UnexpectedJfifVersion { expected: u16, obtained: u16 },
    JfifTooShort { min_expected: usize, obtained: usize },
    SofTooShort { min_expected: usize, obtained: usize },
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e)
                => write!(f, "I/O error: {}", e),
            Self::NotABlock { start_byte }
                => write!(f, "not a block (starting byte 0x{:02X})", start_byte),
            Self::BlockTooShort { min_expected, obtained }
                => write!(f, "block too short -- expected at least {} bytes, obtained {} bytes", min_expected, obtained),
            Self::BlockTooLong { max_allowed, obtained }
                => write!(f, "block too long -- max allowed {} bytes, obtained {} bytes", max_allowed, obtained),
            Self::IncompleteData { builder }
                => write!(f, "incomplete data in header: {:?}", builder),
            Self::UnexpectedBlock { expected_kind, obtained_kind  }
                => write!(f, "unexpected block 0x{:02X} (expected 0x{:02X})", obtained_kind, expected_kind),
            Self::IncorrectImageDataTermination
                => write!(f, "image data terminated incorrectly"),
            Self::NotJfif
                => write!(f, "file is not a JFIF file"),
            Self::UnexpectedJfifVersion { expected, obtained }
                => write!(f, "unexpected JFIF version; expected 0x{:04X}, obtained 0x{:04X}", expected, obtained),
            Self::JfifTooShort { min_expected, obtained }
                => write!(f, "JFIF header too short; expected at least {} bytes, obtained {}", min_expected, obtained),
            Self::SofTooShort { min_expected, obtained }
                => write!(f, "Start-of-Frame too short; expected at least {} bytes, obtained {}", min_expected, obtained),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::NotABlock { .. } => None,
            Self::BlockTooShort { .. } => None,
            Self::BlockTooLong { .. } => None,
            Self::IncompleteData { .. } => None,
            Self::UnexpectedBlock { .. } => None,
            Self::IncorrectImageDataTermination => None,
            Self::NotJfif => None,
            Self::UnexpectedJfifVersion { .. } => None,
            Self::JfifTooShort { .. } => None,
            Self::SofTooShort { .. } => None,
        }
    }
}
impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self { Self::Io(value) }
}


#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Image {
    pub bit_depth: u8,
    pub width: u16,
    pub height: u16,
    pub color_space: ColorSpace,
    pub density_unit: DensityUnit,
    pub density_x: u16,
    pub density_y: u16,
    pub leading_blocks: Vec<Block>,
    pub image_data: Vec<u8>,
    pub trailing_blocks: Vec<Block>,
}
impl Image {
    pub fn try_read<R: Read>(mut reader: R) -> Result<Self, Error> {
        let mut builder = ImageBuilder::new();
        loop {
            let block = Block::try_read(&mut reader)?;
            builder.leading_blocks.push(block);
            let block_ref = builder.leading_blocks.last().unwrap();

            if builder.leading_blocks.len() == 1 {
                // first block must be start-of-image
                if block_ref.kind() != 0xD8 {
                    return Err(Error::UnexpectedBlock {
                        obtained_kind: block_ref.kind(),
                        expected_kind: 0xD8,
                    });
                }
            } else if block_ref.kind() == 0xDA {
                // start-of-scan; the image data follows
                break;
            }
        }

        // read the image data
        let mut image_data = Vec::new();
        reader.read_to_end(&mut image_data)?;

        if image_data.ends_with(&[0xFF, 0xD9]) {
            // ends with end-of-input, perfect
            image_data.drain(image_data.len()-2..);
            builder.trailing_blocks.push(Block::Short { kind: 0xD9 });
        } else {
            return Err(Error::IncorrectImageDataTermination);
        }

        builder.image_data = image_data;

        for block in &builder.leading_blocks {
            let data = block.data();
            match block.kind() {
                0xE0 => {
                    // APP0
                    if !data.starts_with(b"JFIF\0") {
                        return Err(Error::NotJfif);
                    }
                    if data.len() < 12 {
                        return Err(Error::JfifTooShort { min_expected: 12, obtained: data.len() });
                    }

                    let version = u16::from_be_bytes(data[5..7].try_into().unwrap());
                    if version != 0x0101 {
                        return Err(Error::UnexpectedJfifVersion { expected: 0x0101, obtained: version });
                    }

                    let unit = DensityUnit::from_base_type(data[7]);
                    let density_x = u16::from_be_bytes(data[8..10].try_into().unwrap());
                    let density_y = u16::from_be_bytes(data[10..12].try_into().unwrap());

                    builder.density_unit = Some(unit);
                    builder.density_x = Some(density_x);
                    builder.density_y = Some(density_y);
                },
                0xC0..=0xC3|0xC5..=0xC7|0xC9..=0xCB|0xCD..=0xCF => {
                    // start of frame
                    if data.len() < 6 {
                        return Err(Error::SofTooShort { min_expected: 6, obtained: data.len() });
                    }
                    let bit_depth = data[0];
                    let height = u16::from_be_bytes(data[1..3].try_into().unwrap());
                    let width = u16::from_be_bytes(data[3..5].try_into().unwrap());
                    let color_space = ColorSpace::from_base_type(data[5]);
                    builder.bit_depth = Some(bit_depth);
                    builder.height = Some(height);
                    builder.width = Some(width);
                    builder.color_space = Some(color_space);
                },
                _ => {},
            }
        }

        builder.try_into()
    }

    pub fn write<W: Write>(&self, mut writer: W) -> Result<(), Error> {
        for leading_block in &self.leading_blocks {
            leading_block.write(&mut writer)?;
        }
        writer.write_all(&self.image_data)?;
        for trailing_block in &self.trailing_blocks {
            trailing_block.write(&mut writer)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ImageBuilder {
    pub bit_depth: Option<u8>,
    pub width: Option<u16>,
    pub height: Option<u16>,
    pub color_space: Option<ColorSpace>,
    pub density_unit: Option<DensityUnit>,
    pub density_x: Option<u16>,
    pub density_y: Option<u16>,
    pub leading_blocks: Vec<Block>,
    pub image_data: Vec<u8>,
    pub trailing_blocks: Vec<Block>,
}
impl ImageBuilder {
    pub fn new() -> Self {
        Self {
            bit_depth: None,
            width: None,
            height: None,
            color_space: None,
            density_unit: None,
            density_x: None,
            density_y: None,
            leading_blocks: Vec::new(),
            image_data: Vec::new(),
            trailing_blocks: Vec::new(),
        }
    }

    fn build(&self) -> Option<Image> {
        let bit_depth = self.bit_depth?;
        let width = self.width?;
        let height = self.height?;
        let color_space = self.color_space?;
        let density_unit = self.density_unit?;
        let density_x = self.density_x?;
        let density_y = self.density_y?;
        let leading_blocks = self.leading_blocks.clone();
        let image_data = self.image_data.clone();
        let trailing_blocks = self.trailing_blocks.clone();
        Some(Image {
            bit_depth,
            width,
            height,
            color_space,
            density_unit,
            density_x,
            density_y,
            leading_blocks,
            image_data,
            trailing_blocks,
        })
    }
}
impl TryFrom<ImageBuilder> for Image {
    type Error = Error;

    fn try_from(value: ImageBuilder) -> Result<Self, Self::Error> {
        value.build()
            .ok_or_else(|| Error::IncompleteData { builder: value })
    }
}


#[derive(Clone, Copy, Debug)]
#[from_to_other(base_type = u8, derive_compare = "as_int")]
pub enum DensityUnit {
    NoUnit = 0,
    DotsPerInch = 1,
    DotsPerCentimeter = 2,
    Other(u8),
}

#[derive(Clone, Copy, Debug)]
#[from_to_other(base_type = u8, derive_compare = "as_int")]
pub enum ColorSpace {
    Grayscale = 1,
    Rgb = 3,
    Cmyk = 4,
    Other(u8),
}

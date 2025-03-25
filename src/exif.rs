//! Parsing the Extensible Image File Format (Exif).


use std::fmt;
use std::io::{self, Cursor, Read, Seek, SeekFrom};

use from_to_repr::from_to_other;

use crate::jpeg::{DensityUnit, ImageBuilder};


#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    ByteOrderIndicator { bytes: [u8; 2] },
    Version { version: u16 },
    BigPointerSize { size: u16 },
    BigReserved { value: u16 },
    UnknownType { data_type: ValueType },
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e)
                => write!(f, "I/O error: {}", e),
            Self::ByteOrderIndicator { bytes }
                => write!(f, "unrecognized byte order indicator 0x{:02X} 0x{:02X}", bytes[0], bytes[1]),
            Self::Version { version }
                => write!(f, "unknown TIFF version {}", version),
            Self::BigPointerSize { size }
                => write!(f, "unexpected BigTIFF pointer size {}", size),
            Self::BigReserved { value }
                => write!(f, "unexpected BigTIFF reserved value {}", value),
            Self::UnknownType { data_type }
                => write!(f, "unknown data type {:?}", data_type),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::ByteOrderIndicator { .. } => None,
            Self::Version { .. } => None,
            Self::BigPointerSize { .. } => None,
            Self::BigReserved { .. } => None,
            Self::UnknownType { .. } => None,
        }
    }
}
impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self { Self::Io(value) }
}


macro_rules! impl_read {
    ($name:ident, $type:ty, $buf_size:expr) => {
        fn $name(&mut self) -> Result<$type, Error> {
            let mut buf = [0u8; $buf_size];
            self.reader.read_exact(&mut buf)?;
            Ok(if self.big_endian {
                <$type>::from_be_bytes(buf)
            } else {
                <$type>::from_le_bytes(buf)
            })
        }
    };
}


struct Reader<R: Read + Seek> {
    reader: R,
    big_endian: bool,
    ptr64: bool,
}
impl<R: Read + Seek> Reader<R> {
    pub fn new(mut reader: R) -> Result<Self, Error> {
        let mut byte_order_buf = [0u8; 2];
        reader.read_exact(&mut byte_order_buf)?;
        let big_endian = match (byte_order_buf[0], byte_order_buf[1]) {
            (0x4D, 0x4D) => true, // MM (Motorola Motorola)
            (0x49, 0x49) => false, // II (Intel Intel)
            _ => return Err(Error::ByteOrderIndicator { bytes: byte_order_buf }),
        };
        let mut tiff_reader = Self {
            reader,
            big_endian,
            ptr64: false,
        };

        // read 16 bytes
        let version = tiff_reader.read_u16()?;
        match version {
            42 => {
                // standard TIFF
            },
            43 => {
                // BigTIFF
                tiff_reader.ptr64 = true;
            },
            other => return Err(Error::Version { version: other }),
        }

        if tiff_reader.ptr64 {
            // we have a few more fields in the header
            let pointer_size = tiff_reader.read_u16()?;
            if pointer_size != 8 {
                return Err(Error::BigPointerSize { size: pointer_size });
            }

            let reserved = tiff_reader.read_u16()?;
            if reserved != 0 {
                return Err(Error::BigReserved { value: reserved });
            }
        }

        // where is the first directory?
        let dir_offset = tiff_reader.read_offset()?;

        // go there
        tiff_reader.reader.seek(SeekFrom::Start(dir_offset))?;

        Ok(tiff_reader)
    }

    fn read_i8(&mut self) -> Result<i8, Error> {
        let mut buf = [0u8];
        self.reader.read_exact(&mut buf)?;
        Ok(i8::from_ne_bytes(buf))
    }

    impl_read!(read_u16, u16, 2);
    impl_read!(read_u32, u32, 4);
    impl_read!(read_u64, u64, 8);
    impl_read!(read_i16, i16, 2);
    impl_read!(read_i32, i32, 4);
    impl_read!(read_i64, i64, 8);
    impl_read!(read_f32, f32, 4);
    impl_read!(read_f64, f64, 8);

    fn read_offset(&mut self) -> Result<u64, Error> {
        if self.ptr64 {
            self.read_u64()
        } else {
            self.read_u32()
                .map(|i| i.into())
        }
    }

    fn read_ifd_entry_count(&mut self) -> Result<u64, Error> {
        if self.ptr64 {
            self.read_u64()
        } else {
            self.read_u16()
                .map(|i| i.into())
        }
    }

    fn read_type(&mut self) -> Result<ValueType, Error> {
        let base_value = self.read_u16()?;
        Ok(ValueType::from_base_type(base_value))
    }

    fn read_value_or_pointer(&mut self, tag: u16, kind: ValueType, count: u32) -> Result<ValueOrPointer, Error> {
        let mut inline_buf = [0u8; 8];
        let inline_slice = if self.ptr64 {
            self.reader.read_exact(&mut inline_buf)?;
            &inline_buf[..]
        } else {
            self.reader.read_exact(&mut inline_buf[0..4])?;
            &inline_buf[0..4]
        };

        // how much space does this value need?
        let single_value_size = match kind.single_element_size() {
            Some(svs) => svs,
            None => {
                // unknown value type; return the inline bytes raw
                return Ok(ValueOrPointer::Unknown {
                    tag,
                    value_type: kind,
                    count,
                    inline_data: inline_buf,
                });
            },
        };

        // how much space do all the values need?
        let total_size = single_value_size * usize::try_from(count).unwrap();
        if total_size > inline_slice.len() {
            // it's a pointer
            let pointer_value: u64 = match (self.ptr64, self.big_endian) {
                (false, false) => u32::from_le_bytes(inline_buf[0..4].try_into().unwrap()).into(),
                (false, true) => u32::from_be_bytes(inline_buf[0..4].try_into().unwrap()).into(),
                (true, false) => u64::from_le_bytes(inline_buf).into(),
                (true, true) => u64::from_be_bytes(inline_buf).into(),
            };
            Ok(ValueOrPointer::Pointer {
                tag,
                value_type: kind,
                count,
                pointer: pointer_value,
            })
        } else {
            // it fits inline
            let inline_cursor = Cursor::new(inline_slice);
            let mut inline_reader = Reader {
                reader: inline_cursor,
                big_endian: self.big_endian,
                ptr64: self.ptr64,
            };
            let values = inline_reader.read_values(kind, count)?;
            Ok(ValueOrPointer::Value {
                tag,
                values,
            })
        }
    }

    fn read_values(&mut self, kind: ValueType, count: u32) -> Result<Values, Error> {
        let count_usize: usize = count.try_into().unwrap();
        match kind {
            ValueType::Byte|ValueType::Ascii|ValueType::Undefined => {
                let mut buf = vec![0; count_usize];
                self.reader.read_exact(buf.as_mut_slice())?;
                match kind {
                    ValueType::Byte => Ok(Values::Byte(buf)),
                    ValueType::Ascii => Ok(Values::Ascii(buf)),
                    ValueType::Undefined => Ok(Values::Undefined(buf)),
                    _ => unreachable!(),
                }
            },
            ValueType::Short => {
                let mut buf = Vec::with_capacity(count_usize);
                for _ in 0..count_usize {
                    let value = self.read_u16()?;
                    buf.push(value);
                }
                Ok(Values::Short(buf))
            },
            ValueType::Long => {
                let mut buf = Vec::with_capacity(count_usize);
                for _ in 0..count_usize {
                    let value = self.read_u32()?;
                    buf.push(value);
                }
                Ok(Values::Long(buf))
            },
            ValueType::Rational => {
                let mut buf = Vec::with_capacity(count_usize);
                for _ in 0..count_usize {
                    let numerator = self.read_u32()?;
                    let denominator = self.read_u32()?;
                    buf.push((numerator, denominator));
                }
                Ok(Values::Rational(buf))
            },
            ValueType::SByte => {
                let mut buf = Vec::with_capacity(count_usize);
                for _ in 0..count_usize {
                    let value = self.read_i8()?;
                    buf.push(value);
                }
                Ok(Values::SByte(buf))
            },
            ValueType::SShort => {
                let mut buf = Vec::with_capacity(count_usize);
                for _ in 0..count_usize {
                    let value = self.read_i16()?;
                    buf.push(value);
                }
                Ok(Values::SShort(buf))
            },
            ValueType::SLong => {
                let mut buf = Vec::with_capacity(count_usize);
                for _ in 0..count_usize {
                    let value = self.read_i32()?;
                    buf.push(value);
                }
                Ok(Values::SLong(buf))
            },
            ValueType::SRational => {
                let mut buf = Vec::with_capacity(count_usize);
                for _ in 0..count_usize {
                    let numerator = self.read_i32()?;
                    let denominator = self.read_i32()?;
                    buf.push((numerator, denominator));
                }
                Ok(Values::SRational(buf))
            },
            ValueType::Float => {
                let mut buf = Vec::with_capacity(count_usize);
                for _ in 0..count_usize {
                    let value = self.read_f32()?;
                    buf.push(value);
                }
                Ok(Values::Float(buf))
            },
            ValueType::Double => {
                let mut buf = Vec::with_capacity(count_usize);
                for _ in 0..count_usize {
                    let value = self.read_f64()?;
                    buf.push(value);
                }
                Ok(Values::Double(buf))
            },
            ValueType::Ifd => {
                let mut buf = Vec::with_capacity(count_usize);
                for _ in 0..count_usize {
                    let value = self.read_u32()?;
                    buf.push(value);
                }
                Ok(Values::Ifd(buf))
            },
            ValueType::Long8 => {
                let mut buf = Vec::with_capacity(count_usize);
                for _ in 0..count_usize {
                    let value = self.read_u64()?;
                    buf.push(value);
                }
                Ok(Values::Long8(buf))
            },
            ValueType::SLong8 => {
                let mut buf = Vec::with_capacity(count_usize);
                for _ in 0..count_usize {
                    let value = self.read_i64()?;
                    buf.push(value);
                }
                Ok(Values::SLong8(buf))
            },
            ValueType::Ifd8 => {
                let mut buf = Vec::with_capacity(count_usize);
                for _ in 0..count_usize {
                    let value = self.read_u64()?;
                    buf.push(value);
                }
                Ok(Values::Ifd8(buf))
            },
            ValueType::Other(_) => Err(Error::UnknownType { data_type: kind }),
        }
    }
}


#[derive(Clone, Copy, Debug)]
#[from_to_other(base_type = u16, derive_compare = "as_int")]
pub enum ValueType {
    Byte = 1,
    Ascii = 2,
    Short = 3,
    Long = 4,
    Rational = 5,
    SByte = 6,
    Undefined = 7,
    SShort = 8,
    SLong = 9,
    SRational = 10,
    Float = 11,
    Double = 12,
    Ifd = 13,
    // Unicode = 14, Complex = 15, Adobe-internal assignments
    Long8 = 16,
    SLong8 = 17,
    Ifd8 = 18,
    Other(u16),
}
impl ValueType {
    pub fn single_element_size(&self) -> Option<usize> {
        match self {
            Self::Byte|Self::Ascii|Self::SByte|Self::Undefined => Some(1),
            Self::Short|Self::SShort => Some(2),
            Self::Long|Self::SLong|Self::Float|Self::Ifd => Some(4),
            Self::Rational|Self::SRational|Self::Double|Self::Long8|Self::SLong8|Self::Ifd8 => Some(8),
            Self::Other(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub enum Values {
    Byte(Vec<u8>),
    Ascii(Vec<u8>),
    Short(Vec<u16>),
    Long(Vec<u32>),
    Rational(Vec<(u32, u32)>),
    SByte(Vec<i8>),
    Undefined(Vec<u8>),
    SShort(Vec<i16>),
    SLong(Vec<i32>),
    SRational(Vec<(i32, i32)>),
    Float(Vec<f32>),
    Double(Vec<f64>),
    Ifd(Vec<u32>),
    Long8(Vec<u64>),
    SLong8(Vec<i64>),
    Ifd8(Vec<u64>),
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub enum ValueOrPointer {
    Value { tag: u16, values: Values },
    Pointer { tag: u16, value_type: ValueType, count: u32, pointer: u64 },
    Unknown { tag: u16, value_type: ValueType, count: u32, inline_data: [u8; 8] },
}
impl ValueOrPointer {
    pub fn tag(&self) -> u16 {
        match self {
            Self::Value { tag, .. } => *tag,
            Self::Pointer { tag, .. } => *tag,
            Self::Unknown { tag, .. } => *tag,
        }
    }

    pub fn value(&self) -> Option<&Values> {
        match self {
            Self::Value { values, .. } => Some(values),
            _ => None,
        }
    }
}


pub(crate) fn process(app1_data: &[u8], builder: &mut ImageBuilder) -> Result<(), Error> {
    assert!(app1_data.starts_with(b"Exif\0\0"));
    let exif_tiff = &app1_data[6..];
    let tiff_cursor = Cursor::new(exif_tiff);
    let mut tiff = Reader::new(tiff_cursor)?;

    let mut ifds_values = Vec::new();

    loop {
        // how many entries in the IFD do we have?
        let ifd_entry_count = tiff.read_ifd_entry_count()?;
        let mut values = Vec::new();

        // run through them, collecting the values
        for _ in 0..ifd_entry_count {
            let tag = tiff.read_u16()?;
            let kind = tiff.read_type()?;
            let count = tiff.read_u32()?;

            let value_or_pointer = tiff.read_value_or_pointer(tag, kind, count)?;
            values.push(value_or_pointer);
        }

        ifds_values.push(values);

        // the next value is the pointer to the next IFD
        let next_ifd_offset = tiff.read_offset()?;
        if next_ifd_offset == 0 {
            // we are done
            break;
        }

        tiff.reader.seek(SeekFrom::Start(next_ifd_offset))?;
    }

    // dereference the pointers
    for values in &mut ifds_values {
        for value in values {
            if let ValueOrPointer::Pointer { tag, value_type, count, pointer } = value {
                tiff.reader.seek(SeekFrom::Start(*pointer))?;
                let values = tiff.read_values(*value_type, *count)?;
                *value = ValueOrPointer::Value { tag: *tag, values };
            }
        }
    }

    // process what we know
    // IFD0 = image itself, IFD1 = thumbnail
    // => ignore IFD1

    // do we have an X resolution? fall back to 72 if not
    let x_resolution_values_opt = ifds_values[0]
        .iter()
        .filter(|v| v.tag() == 0x011A)
        .filter_map(|v| v.value())
        .nth(0);
    let x_resolution_opt = if let Some(x_resolution_values) = x_resolution_values_opt {
        if let Values::Rational(vals) = x_resolution_values {
            if vals.len() == 1 {
                Some(vals[0].0 / vals[0].1)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    let x_resolution = x_resolution_opt.unwrap_or(72);

    // do we have a Y resolution? fall back to X resolution if not
    let y_resolution_values_opt = ifds_values[0]
        .iter()
        .filter(|v| v.tag() == 0x011B)
        .filter_map(|v| v.value())
        .nth(0);
    let y_resolution_opt = if let Some(y_resolution_values) = y_resolution_values_opt {
        if let Values::Rational(vals) = y_resolution_values {
            if vals.len() == 1 {
                Some(vals[0].0 / vals[0].1)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    let y_resolution = y_resolution_opt.unwrap_or(x_resolution);

    // find the unit (fall back to inches)
    let unit_values_opt = ifds_values[0]
        .iter()
        .filter(|v| v.tag() == 0x0128)
        .filter_map(|v| v.value())
        .nth(0);
    let unit_opt = if let Some(unit_values) = unit_values_opt {
        if let Values::Short(vals) = unit_values {
            if vals.len() == 1 {
                Some(vals[0])
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    let unit = unit_opt.unwrap_or(2);

    builder.density_x = Some(x_resolution.try_into().unwrap());
    builder.density_y = Some(y_resolution.try_into().unwrap());
    builder.density_unit = Some(match unit {
        2 => DensityUnit::DotsPerInch,
        3 => DensityUnit::DotsPerCentimeter,
        _ => DensityUnit::DotsPerInch,
    });

    Ok(())
}

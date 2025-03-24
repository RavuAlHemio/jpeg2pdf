use std::{collections::BTreeMap, io::{self, Seek, Write}};

use crate::jpeg::ColorSpace;


pub type PdfObjectId = u64;


#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Document {
    pub objects: BTreeMap<PdfObjectId, ObjectData>,
}
impl Document {
    pub fn write<W: Seek + Write>(&self, mut writer: W) -> Result<(), io::Error> {
        let pdf_start_pos = writer.stream_position()?;

        write!(writer, "%PDF-1.5\n")?;
        // binary detection comment line
        writer.write_all(&[b'%', 0xE2, 0xE3, 0xCF, 0xD3, b'\n'])?;

        // output each object
        let mut xref_offsets = BTreeMap::new();
        for (&id, data) in &self.objects {
            let object_start_pos = writer.stream_position()?;
            xref_offsets.insert(id, object_start_pos - pdf_start_pos);
            write!(writer, "{} 0 obj\n", id)?;
            data.write_to_pdf(&mut writer)?;
            write!(writer, "endobj\n")?;
        }

        let max_obj_id = self.objects.keys()
            .copied()
            .max()
            .expect("no objects");

        let xref_pos = writer.stream_position()?;
        write!(writer, "xref\n")?;
        write!(writer, "0 {}\n", max_obj_id + 1)?;
        let mut cur_obj_id = 0;
        for (&id, &xref_offset) in &xref_offsets {
            while cur_obj_id < id {
                write!(writer, "{:010} 65535 f\r\n", xref_offset)?;
                cur_obj_id += 1;
            }
            write!(writer, "{:010} 00000 n\r\n", xref_offset)?;
            cur_obj_id += 1;
        }

        let root_obj_id = self.objects.iter()
            .filter(|(_id, data)| matches!(data, ObjectData::Catalog(_)))
            .map(|(id, _data)| *id)
            .nth(0)
            .expect("no catalog object found");

        write!(writer, "trailer\n")?;
        write!(writer, "<< /Size {}", max_obj_id + 1)?;
        write!(writer, " /Root {} 0 R", root_obj_id)?;
        write!(writer, " >>\n")?;
        write!(writer, "startxref\n")?;
        write!(writer, "{}\n", xref_pos - pdf_start_pos)?;
        write!(writer, "%%EOF\n")?;

        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ObjectData {
    Catalog(Catalog),
    Pages(Pages),
    Page(Page),
    PageResources(PageResources),
    PageContents(PageContents),
    ImageXObject(ImageXObject),
}
impl ObjectData {
    pub fn write_to_pdf<W: Write>(&self, writer: W) -> Result<(), io::Error> {
        match self {
            Self::Catalog(obj) => obj.write_to_pdf(writer),
            Self::Page(obj) => obj.write_to_pdf(writer),
            Self::Pages(obj) => obj.write_to_pdf(writer),
            Self::PageResources(obj) => obj.write_to_pdf(writer),
            Self::PageContents(obj) => obj.write_to_pdf(writer),
            Self::ImageXObject(obj) => obj.write_to_pdf(writer),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Catalog {
    pub root_page_id: PdfObjectId,
}
impl Catalog {
    pub fn write_to_pdf<W: Write>(&self, mut writer: W) -> Result<(), io::Error> {
        write!(writer, "<< /Type /Catalog")?;
        write!(writer, " /Pages {} 0 R", self.root_page_id)?;
        write!(writer, " >>\n")
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Pages {
    pub page_ids: Vec<PdfObjectId>,
}
impl Pages {
    pub fn write_to_pdf<W: Write>(&self, mut writer: W) -> Result<(), io::Error> {
        write!(writer, "<< /Type /Pages")?;
        write!(writer, " /Kids [")?;
        for &page_id in &self.page_ids {
            write!(writer, " {} 0 R", page_id)?;
        }
        write!(writer, " ]")?;
        write!(writer, " /Count {}", self.page_ids.len())?;
        write!(writer, " >>\n")
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Page {
    pub parent_id: PdfObjectId,
    pub resources_id: PdfObjectId,
    pub contents_id: PdfObjectId,
    pub width_pt: u64,
    pub height_pt: u64,
}
impl Page {
    pub fn write_to_pdf<W: Write>(&self, mut writer: W) -> Result<(), io::Error> {
        write!(writer, "<< /Type /Page")?;
        write!(writer, " /Parent {} 0 R", self.parent_id)?;
        write!(writer, " /Resources {} 0 R", self.resources_id)?;
        write!(writer, " /MediaBox [ 0 0 {} {} ]", self.width_pt, self.height_pt)?;
        write!(writer, " /Contents {} 0 R", self.contents_id)?;
        write!(writer, " >>\n")
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PageResources {
    pub image_xobject_ids: Vec<PdfObjectId>,
}
impl PageResources {
    pub fn write_to_pdf<W: Write>(&self, mut writer: W) -> Result<(), io::Error> {
        write!(writer, "<< /ProcSet [ /PDF /Text /ImageB /ImageC /ImageI ]")?;
        write!(writer, " /XObject <<")?;
        for (image_index, image_xobject_id) in self.image_xobject_ids.iter().copied().enumerate() {
            write!(writer, " /Im{} {} 0 R", image_index, image_xobject_id)?;
        }
        write!(writer, " >>")?;
        write!(writer, " >>\n")
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PageContents {
    pub commands: String,
}
impl PageContents {
    pub fn write_to_pdf<W: Write>(&self, mut writer: W) -> Result<(), io::Error> {
        write!(writer, "<< /Length {} >>\n", self.commands.len())?;
        write!(writer, "stream\n")?;
        write!(writer, "{}", self.commands)?;
        write!(writer, "\nendstream\n")
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ImageXObject {
    pub width: u64,
    pub height: u64,
    pub color_space: &'static str,
    pub bits_per_component: u8,
    pub interpolate: bool,
    pub data_filters: Vec<String>,
    pub data: Vec<u8>,
}
impl ImageXObject {
    pub fn from_jpeg_image(jpeg_image: &crate::jpeg::Image) -> Option<Self> {
        let width = jpeg_image.width.into();
        let height = jpeg_image.height.into();
        let color_space = match jpeg_image.color_space {
            ColorSpace::Grayscale => "/DeviceGray",
            ColorSpace::Rgb => "/DeviceRGB",
            ColorSpace::Cmyk => "/DeviceCMYK",
            ColorSpace::Other(_) => return None,
        };
        let bits_per_component = jpeg_image.bit_depth;
        let interpolate = false;
        let data_filters = vec!["/DCTDecode".to_owned()];
        let mut data = Vec::new();
        jpeg_image.write(&mut data).ok()?;
        Some(Self {
            width,
            height,
            color_space,
            bits_per_component,
            interpolate,
            data_filters,
            data,
        })
    }

    pub fn write_to_pdf<W: Write>(&self, mut writer: W) -> Result<(), io::Error> {
        write!(writer, "<< /Type /XObject /Subtype /Image")?;
        write!(writer, " /Width {}", self.width)?;
        write!(writer, " /Height {}", self.height)?;
        write!(writer, " /ColorSpace {}", self.color_space)?;
        write!(writer, " /BitsPerComponent {}", self.bits_per_component)?;
        if self.data_filters.len() > 0 {
            write!(writer, " /Filter [")?;
            for filter in &self.data_filters {
                write!(writer, " {}", filter)?;
            }
            write!(writer, " ]")?;
        }
        write!(writer, " /Length {}", self.data.len())?;
        write!(writer, " >>\nstream\n")?;
        writer.write_all(&self.data)?;
        write!(writer, "\nendstream\n")
    }
}

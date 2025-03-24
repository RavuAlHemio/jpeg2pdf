mod jpeg;
mod pdf;


use std::collections::BTreeMap;
use std::fs::File;
use std::path::PathBuf;

use clap::Parser;

use crate::jpeg::{ColorSpace, DensityUnit};


#[derive(Parser)]
struct Opts {
    #[arg(short, long)]
    remove_optional_metadata: bool,

    input_jpeg_path: PathBuf,
    output_pdf_path: PathBuf,
}


fn main() {
    let opts = Opts::parse();

    // crunch the JPEG
    let jpeg_file = File::open(&opts.input_jpeg_path)
        .expect("failed to open input JPEG file");
    let mut jpeg = jpeg::Image::try_read(jpeg_file)
        .expect("failed to read JPEG file");

    if jpeg.bit_depth != 8 {
        panic!("JPEG bit depth {} is not supported; only 8 bits per component", jpeg.bit_depth);
    }
    if let ColorSpace::Other(n) = jpeg.color_space {
        panic!("color space {} is not supported (only 1=Grayscale, 2=RGB, 3=CMYK)", n);
    }

    if opts.remove_optional_metadata {
        // remove unimportant leading blocks
        jpeg.leading_blocks.retain(|b| b.is_required());
    }

    // default user space unit: 1/72 inch (Adobe point)
    let (width_pt, height_pt) = match jpeg.density_unit {
        DensityUnit::NoUnit => panic!("no density unit specified; don't know how to size page"),
        DensityUnit::Other(u) => panic!("unknown density unit {}", u),
        DensityUnit::DotsPerInch => {
            let width_pt = (u64::from(jpeg.width) * 72) / u64::from(jpeg.density_x);
            let height_pt = (u64::from(jpeg.height) * 72) / u64::from(jpeg.density_y);
            (width_pt, height_pt)
        },
        DensityUnit::DotsPerCentimeter => {
            let width_pt = (u64::from(jpeg.width) * 7200) / (u64::from(jpeg.density_x) * 254);
            let height_pt = (u64::from(jpeg.height) * 7200) / (u64::from(jpeg.density_y) * 254);
            (width_pt, height_pt)
        },
    };

    // PDF document structure:
    // 1 = catalog
    // 2 = pages
    // 3 = page
    // 4 = page resources
    // 5 = page contents
    // 6 = image

    let catalog = pdf::ObjectData::Catalog(pdf::Catalog {
        root_page_id: 2,
    });
    let pages = pdf::ObjectData::Pages(pdf::Pages {
        page_ids: vec![3],
    });
    let page = pdf::ObjectData::Page(pdf::Page {
        parent_id: 2,
        resources_id: 4,
        contents_id: 5,
        width_pt,
        height_pt,
    });
    let resources = pdf::ObjectData::PageResources(pdf::PageResources {
        image_xobject_ids: vec![6],
    });
    let contents = pdf::ObjectData::PageContents(pdf::PageContents {
        commands: format!("q {} 0 0 {} 0 0 cm /Im0 Do Q", width_pt, height_pt),
    });
    let image = pdf::ObjectData::ImageXObject(pdf::ImageXObject::from_jpeg_image(&jpeg).unwrap());

    let mut pdf = pdf::Document {
        objects: BTreeMap::new(),
    };
    pdf.objects.insert(1, catalog);
    pdf.objects.insert(2, pages);
    pdf.objects.insert(3, page);
    pdf.objects.insert(4, resources);
    pdf.objects.insert(5, contents);
    pdf.objects.insert(6, image);

    let output = File::create(&opts.output_pdf_path)
        .expect("failed to create output PDF file");
    pdf.write(output)
        .expect("failed to write output PDF file");
}

//! Decode/encode bytes, avoiding narrow OS-path APIs and empty-image pipelines.
use crate::{Result,digest};
use serde::{Deserialize,Serialize};
use std::io::Cursor;

#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(rename_all="camelCase")]
pub struct ImageInfo {pub width:u32,pub height:u32,pub channels:u8,pub sha256:String,pub format:String}

fn decode(bytes:&[u8])->Result<(image::DynamicImage,image::ImageFormat)> {
    if bytes.is_empty()||bytes.len()>32*1024*1024 {return Err("Image input is empty or exceeds 32 MiB".into());}
    let mut reader=image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e|e.to_string())?;
    let format=reader.format().ok_or("Unsupported image encoding")?;
    let mut limits=image::Limits::default();limits.max_image_width=Some(20_000);limits.max_image_height=Some(20_000);limits.max_alloc=Some(256*1024*1024);reader.limits(limits);
    let image=reader.decode().map_err(|e|format!("Image decode failed: {e}"))?;
    if image.width()==0||image.height()==0 {return Err("Decoded image is empty".into());}
    Ok((image,format))
}
pub fn inspect_image(bytes:&[u8])->Result<ImageInfo> {
    let (image,format)=decode(bytes)?;
    Ok(ImageInfo{width:image.width(),height:image.height(),channels:image.color().channel_count(),sha256:digest(bytes),format:format!("{format:?}").to_ascii_lowercase()})
}
pub fn transcode_image(bytes:&[u8],format:&str,bounds:Option<(u32,u32)>)->Result<(Vec<u8>,ImageInfo)> {
    let (mut image,_)=decode(bytes)?;
    let output=match format {"png"=>image::ImageFormat::Png,"jpeg"=>image::ImageFormat::Jpeg,_=>return Err("Output format must be png or jpeg".into())};
    if let Some((width,height))=bounds {
        if width==0||height==0||width>20_000||height>20_000 {return Err("Resize bounds must be 1–20000 pixels".into());}
        if image.width()>width||image.height()>height {image=image.resize(width,height,image::imageops::FilterType::Lanczos3);}
    }
    if output==image::ImageFormat::Jpeg&&image.color().has_alpha() {return Err("JPEG cannot preserve alpha; choose PNG or explicitly composite the image before conversion".into());}
    let mut encoded=Cursor::new(Vec::new());image.write_to(&mut encoded,output).map_err(|e|format!("Image encode failed: {e}"))?;
    let bytes=encoded.into_inner();let verified=inspect_image(&bytes)?;Ok((bytes,verified))
}
pub fn verify_mask_shape(image:(u32,u32),mask:(u32,u32))->Result<()> {
    if image.0==0||image.1==0||image!=mask {return Err(format!("Image/mask dimensions differ or are empty: {image:?} vs {mask:?}"));}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn png()->Vec<u8> {let mut data=Cursor::new(Vec::new());image::DynamicImage::new_rgb8(4,3).write_to(&mut data,image::ImageFormat::Png).unwrap();data.into_inner()}
    #[test] fn roundtrip_is_byte_based_and_has_real_dimensions() {let (bytes,info)=transcode_image(&png(),"png",Some((2,2))).unwrap();assert_eq!((info.width,info.height),(2,2));assert_eq!(inspect_image(&bytes).unwrap(),info);}
    #[test] fn empty_corrupt_and_wrong_mask_fail_before_processing() {assert!(inspect_image(&[]).is_err());assert!(inspect_image(b"broken").is_err());assert!(verify_mask_shape((2,3),(2,2)).is_err());assert!(verify_mask_shape((2,3),(2,3)).is_ok());}
    #[test] fn alpha_is_not_silently_lost_in_jpeg_conversion() {let mut data=Cursor::new(Vec::new());image::DynamicImage::new_rgba8(1,1).write_to(&mut data,image::ImageFormat::Png).unwrap();assert!(transcode_image(&data.into_inner(),"jpeg",None).unwrap_err().contains("alpha"));}
}

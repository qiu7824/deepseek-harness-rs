//! Bounded Unicode clipboard operations, separately authorized by the Host.
use windows_sys::Win32::{System::{DataExchange::*,Memory::*},Foundation::GlobalFree};
const UNICODE_TEXT:u32=13;
struct Open;
impl Drop for Open{fn drop(&mut self){unsafe{CloseClipboard();}}}
fn open()->Result<Open,String>{if unsafe{OpenClipboard(std::ptr::null_mut())}==0{Err("COMPUTER_USE_CLIPBOARD_BUSY".into())}else{Ok(Open)}}
pub fn read()->Result<String,String>{
    let _guard=open()?;
    unsafe{
        let memory=GetClipboardData(UNICODE_TEXT);
        if memory.is_null(){return Err("COMPUTER_USE_CLIPBOARD_TEXT_UNAVAILABLE".into());}
        let size=GlobalSize(memory);
        if size>131074{return Err("COMPUTER_USE_CLIPBOARD_TOO_LARGE".into());}
        let pointer=GlobalLock(memory) as *const u16;
        if pointer.is_null(){return Err("COMPUTER_USE_CLIPBOARD_READ_FAILED".into());}
        let units=std::slice::from_raw_parts(pointer,size/2);
        let length=units.iter().position(|unit|*unit==0).unwrap_or(units.len());
        let text=String::from_utf16_lossy(&units[..length]);GlobalUnlock(memory);
        if text.len()>65536{return Err("COMPUTER_USE_CLIPBOARD_TOO_LARGE".into());}Ok(text)
    }
}
pub fn write(text:&str)->Result<(),String>{
    if text.len()>65536||text.contains('\0'){return Err("COMPUTER_USE_CLIPBOARD_TEXT_INVALID".into());}
    let units=text.encode_utf16().chain(std::iter::once(0)).collect::<Vec<_>>();
    let _guard=open()?;
    unsafe{
        let memory=GlobalAlloc(GMEM_MOVEABLE,units.len()*2);
        if memory.is_null(){return Err("COMPUTER_USE_CLIPBOARD_WRITE_FAILED".into());}
        let pointer=GlobalLock(memory) as *mut u16;
        if pointer.is_null(){GlobalFree(memory);return Err("COMPUTER_USE_CLIPBOARD_WRITE_FAILED".into());}
        std::ptr::copy_nonoverlapping(units.as_ptr(),pointer,units.len());GlobalUnlock(memory);
        if EmptyClipboard()==0||SetClipboardData(UNICODE_TEXT,memory).is_null(){GlobalFree(memory);return Err("COMPUTER_USE_CLIPBOARD_WRITE_FAILED".into());}
    }Ok(())
}

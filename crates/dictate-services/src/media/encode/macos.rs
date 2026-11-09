//! AudioToolbox as the encoder of audio.m4a.
//!
//! An `ExtAudioFile` takes the blocks as 32-bit floats, encodes them to AAC with the system's encoder and writes the
//! MP4 file as they come, all in one object. There is no muxer to add, and the encoder is always there.

use std::ffi::c_void;
use std::io;
use std::path::{Path, PathBuf};
use std::ptr::{self, NonNull};

use objc2_audio_toolbox::{
    kAudioConverterEncodeBitRate, kAudioFileM4AType, kExtAudioFileProperty_AudioConverter,
    kExtAudioFileProperty_ClientDataFormat, kExtAudioFileProperty_ConverterConfig,
    AudioConverterRef, AudioConverterSetProperty, AudioFileFlags, ExtAudioFileCreateWithURL,
    ExtAudioFileDispose, ExtAudioFileGetProperty, ExtAudioFileRef, ExtAudioFileSetProperty,
    ExtAudioFileWrite,
};
use objc2_core_audio_types::{
    kAudioFormatFlagsNativeFloatPacked, kAudioFormatLinearPCM, kAudioFormatMPEG4AAC, AudioBuffer,
    AudioBufferList, AudioStreamBasicDescription, MPEG4ObjectID,
};
use objc2_core_foundation::CFURL;

use super::{part_path, publish, remove_part, EncodeError, M4aSink, M4A_BITRATE, M4A_RATE};

/// The most frames given to the encoder in one call. A block is cut into pieces of this size, which keeps the
/// encoder's own work buffers small however long the block is.
const MAX_FRAMES: usize = 1 << 16;
/// The number of samples in a packet of AAC.
const AAC_FRAMES_PER_PACKET: u32 = 1024;
const F32_BYTES: u32 = std::mem::size_of::<f32>() as u32;

/// Starts writing `out` through AudioToolbox.
pub(super) fn create(out: &Path) -> Result<Box<dyn M4aSink>, EncodeError> {
    let part = part_path(out);
    let failed = |what: &str, status: i32| EncodeError::Failed(describe(what, status));
    let url = CFURL::from_file_path(&part)
        .ok_or_else(|| EncodeError::Failed("the file's path is not a valid one".into()))?;

    // What the file holds: AAC-LC, mono, 48 kHz.
    let mut file_format = AudioStreamBasicDescription {
        mSampleRate: f64::from(M4A_RATE),
        mFormatID: kAudioFormatMPEG4AAC,
        mFormatFlags: MPEG4ObjectID::AAC_LC.0 as u32,
        mBytesPerPacket: 0,
        mFramesPerPacket: AAC_FRAMES_PER_PACKET,
        mBytesPerFrame: 0,
        mChannelsPerFrame: 1,
        mBitsPerChannel: 0,
        mReserved: 0,
    };
    let mut file: ExtAudioFileRef = ptr::null_mut();
    // SAFETY: the pointers are to values that live through the call. `EraseFile` replaces a `.part` file left by an
    // earlier run.
    let status = unsafe {
        ExtAudioFileCreateWithURL(
            &url,
            kAudioFileM4AType,
            NonNull::from(&mut file_format),
            ptr::null(),
            AudioFileFlags::EraseFile.0,
            NonNull::from(&mut file),
        )
    };
    if status != 0 || file.is_null() {
        return Err(failed("creating the file", status));
    }
    // From here on the sink owns the file: it is closed, and the `.part` file removed, if anything below fails.
    let mut sink = MacSink {
        file,
        out: out.to_owned(),
        part,
        published: false,
    };

    // What the blocks are: 32-bit floats, mono, 48 kHz. The ExtAudioFile converts them.
    let mut client_format = AudioStreamBasicDescription {
        mSampleRate: f64::from(M4A_RATE),
        mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kAudioFormatFlagsNativeFloatPacked,
        mBytesPerPacket: F32_BYTES,
        mFramesPerPacket: 1,
        mBytesPerFrame: F32_BYTES,
        mChannelsPerFrame: 1,
        mBitsPerChannel: 8 * F32_BYTES,
        mReserved: 0,
    };
    // SAFETY: `file` is open, and the value is as big as the size says.
    let status = unsafe {
        ExtAudioFileSetProperty(
            sink.file,
            kExtAudioFileProperty_ClientDataFormat,
            std::mem::size_of::<AudioStreamBasicDescription>() as u32,
            NonNull::from(&mut client_format).cast(),
        )
    };
    if status != 0 {
        return Err(failed("setting the format of the blocks", status));
    }

    sink.set_bit_rate()
        .map_err(|(what, status)| failed(what, status))?;
    Ok(Box::new(sink))
}

/// A file being written by AudioToolbox.
struct MacSink {
    /// The open file; null once it is closed.
    file: ExtAudioFileRef,
    out: PathBuf,
    part: PathBuf,
    /// The `.part` file has become `out`.
    published: bool,
}

// SAFETY: an ExtAudioFile is not tied to the thread that made it, and it is only ever used through `&mut self`.
unsafe impl Send for MacSink {}

impl MacSink {
    /// Sets the bit rate of the encoder (its own default is lower), and tells the file to take it.
    fn set_bit_rate(&mut self) -> Result<(), (&'static str, i32)> {
        let mut converter: AudioConverterRef = ptr::null_mut();
        let mut size = std::mem::size_of::<AudioConverterRef>() as u32;
        // SAFETY: `file` is open; the value is as big as the size says.
        let status = unsafe {
            ExtAudioFileGetProperty(
                self.file,
                kExtAudioFileProperty_AudioConverter,
                NonNull::from(&mut size),
                NonNull::from(&mut converter).cast(),
            )
        };
        if status != 0 || converter.is_null() {
            return Err(("finding the encoder", status));
        }
        let mut bit_rate = M4A_BITRATE;
        // SAFETY: `converter` belongs to the open file, which outlives the call.
        let status = unsafe {
            AudioConverterSetProperty(
                converter,
                kAudioConverterEncodeBitRate,
                std::mem::size_of::<u32>() as u32,
                NonNull::from(&mut bit_rate).cast(),
            )
        };
        if status != 0 {
            return Err(("setting the bit rate", status));
        }
        // The file reads the encoder's settings again when its configuration is set (to nothing).
        let mut no_config: *const c_void = ptr::null();
        // SAFETY: `file` is open; the value is a pointer, as big as the size says.
        let status = unsafe {
            ExtAudioFileSetProperty(
                self.file,
                kExtAudioFileProperty_ConverterConfig,
                std::mem::size_of::<*const c_void>() as u32,
                NonNull::from(&mut no_config).cast(),
            )
        };
        if status != 0 {
            return Err(("applying the bit rate", status));
        }
        Ok(())
    }

    /// Closes the file, which makes the encoder write what it still holds and the index of the MP4 file. Does
    /// nothing the second time.
    fn close(&mut self) -> i32 {
        if self.file.is_null() {
            return 0;
        }
        // SAFETY: `file` is open, and is forgotten here so that it is closed only once.
        let status = unsafe { ExtAudioFileDispose(self.file) };
        self.file = ptr::null_mut();
        status
    }
}

impl M4aSink for MacSink {
    fn write(&mut self, mono_48k: &[f32]) -> io::Result<()> {
        for piece in mono_48k.chunks(MAX_FRAMES) {
            let mut buffers = AudioBufferList {
                mNumberBuffers: 1,
                mBuffers: [AudioBuffer {
                    mNumberChannels: 1,
                    mDataByteSize: piece.len() as u32 * F32_BYTES,
                    // The encoder only reads from it.
                    mData: piece.as_ptr().cast_mut().cast::<c_void>(),
                }],
            };
            // SAFETY: `file` is open, and `buffers` describes `piece`, which outlives the call.
            let status = unsafe {
                ExtAudioFileWrite(self.file, piece.len() as u32, NonNull::from(&mut buffers))
            };
            if status != 0 {
                return Err(io::Error::other(describe("encoding", status)));
            }
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> io::Result<()> {
        let status = self.close();
        if status != 0 {
            return Err(io::Error::other(describe("finishing the file", status)));
        }
        publish(&self.part, &self.out)?;
        self.published = true;
        Ok(())
    }
}

impl Drop for MacSink {
    /// A sink that is dropped without being finished leaves no file.
    fn drop(&mut self) {
        self.close();
        if !self.published {
            remove_part(&self.part);
        }
    }
}

/// What went wrong in words: AudioToolbox's status is a number, and often four characters too (`!dat`, `fmt?`).
fn describe(what: &str, status: i32) -> String {
    let code = status.to_be_bytes();
    if code.iter().all(|b| b.is_ascii_graphic()) {
        format!(
            "AudioToolbox failed {what}: {status} ({})",
            String::from_utf8_lossy(&code)
        )
    } else {
        format!("AudioToolbox failed {what}: {status}")
    }
}

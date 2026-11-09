//! Media Foundation as the encoder of audio.m4a: the Sink Writer takes 16-bit PCM, encodes it to AAC with the AAC
//! encoder that ships with Windows, and writes the MP4 file as the blocks come.
//!
//! The "N" editions of Windows have no Media Foundation unless the Media Feature Pack is installed, and a program that
//! imports its DLLs does not start without them. So nothing here is imported: the DLLs are loaded (from the system
//! folder) when the first sink is made, and the few functions that are not methods of an interface are looked up in
//! them. When that fails the sink is not made, and the caller goes on to ffmpeg.

use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::OnceLock;
use std::thread::{self, ThreadId};

use windows::core::{s, w, Interface, HRESULT, PCSTR, PCWSTR};
use windows::Win32::Foundation::{E_FAIL, HMODULE, RPC_E_CHANGED_MODE};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFByteStream, IMFMediaBuffer, IMFMediaType, IMFSample, IMFSinkWriter,
    MFAudioFormat_AAC, MFAudioFormat_PCM, MFMediaType_Audio, MFTranscodeContainerType_MPEG4,
    MFSTARTUP_LITE, MF_ACCESSMODE_WRITE, MF_FILEFLAGS_NONE,
    MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, MF_MT_AAC_PAYLOAD_TYPE,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MF_OPENMODE_DELETE_IF_EXIST, MF_TRANSCODE_CONTAINERTYPE, MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};

use super::{part_path, publish, remove_part, EncodeError, M4aSink, M4A_BITRATE, M4A_RATE};

/// How many samples go to the encoder in one piece: a block is cut into pieces of this size (a second).
const PIECE_SAMPLES: usize = M4A_RATE as usize;
/// The unit of Media Foundation's times: 100 ns.
const TICKS_PER_SECOND: u64 = 10_000_000;
/// The "AAC Profile L2" of the MPEG-4 audio profile and level numbers: AAC-LC up to 48 kHz and two channels.
const AAC_PROFILE_LEVEL_L2: u32 = 0x29;

/// Starts writing `out` through Media Foundation. `Unavailable` when this Windows has none.
pub(super) fn create(out: &Path) -> Result<Box<dyn M4aSink>, EncodeError> {
    let api = Api::get().ok_or(EncodeError::Unavailable)?;
    let part = part_path(out);
    let mut wide: Vec<u16> = part.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(EncodeError::Failed(
            "the file's path is not a valid one".into(),
        ));
    }
    wide.push(0);

    // Both are undone, in the opposite order, when the sink is dropped (or here, if something fails).
    let com = Com::start().map_err(|e| failed("starting COM", e))?;
    let startup = Startup::new(api).map_err(|e| failed("starting up", e))?;

    // SAFETY: Media Foundation is started, and `wide` ends in a zero.
    let opened = unsafe { open_sink(api, &wide) };
    // The file is made before the writer, which is where a Windows without the AAC encoder gives up: it is left over.
    // (Whatever `open_sink` made is let go of by now.)
    if opened.is_err() {
        remove_part(&part);
    }
    let (writer, byte_stream, stream) = opened?;

    Ok(Box::new(MfSink {
        writer: Some(writer),
        byte_stream: Some(byte_stream),
        api,
        stream,
        frames: 0,
        out: out.to_owned(),
        part,
        published: false,
        _startup: startup,
        _com: com,
    }))
}

fn failed(what: &str, error: windows::core::Error) -> EncodeError {
    EncodeError::Failed(format!("Media Foundation failed {what}: {error}"))
}

/// A started writer, the byte stream of the file it writes to, and the number of the stream of the file that the audio
/// goes to.
///
/// # Safety
/// Media Foundation has to be started, and `path` has to end in a zero.
unsafe fn open_sink(
    api: &Api,
    path: &[u16],
) -> Result<(IMFSinkWriter, IMFByteStream, u32), EncodeError> {
    // SAFETY: as the caller says.
    unsafe {
        let byte_stream = open_file(api, path).map_err(|e| failed("creating the file", e))?;
        let writer = make_writer(api, &byte_stream).map_err(|e| failed("making the writer", e))?;
        let stream = begin(api, &writer).map_err(|(what, e)| failed(what, e))?;
        Ok((writer, byte_stream, stream))
    }
}

/// A new file for writing, as a byte stream. `path` ends in a zero.
///
/// # Safety
/// Media Foundation has to be started.
unsafe fn open_file(api: &Api, path: &[u16]) -> windows::core::Result<IMFByteStream> {
    let mut raw = ptr::null_mut();
    // SAFETY: as the caller says; `path` ends in a zero.
    unsafe {
        take(
            (api.create_file)(
                MF_ACCESSMODE_WRITE.0,
                MF_OPENMODE_DELETE_IF_EXIST.0,
                MF_FILEFLAGS_NONE.0,
                PCWSTR(path.as_ptr()),
                &mut raw,
            ),
            raw,
        )
    }
}

/// A Sink Writer that writes an MP4 file to `byte_stream`. The container is named, because the name of the file, which
/// ends in `.part`, says nothing of it.
///
/// # Safety
/// Media Foundation has to be started.
unsafe fn make_writer(
    api: &Api,
    byte_stream: &IMFByteStream,
) -> windows::core::Result<IMFSinkWriter> {
    let mut raw = ptr::null_mut();
    // SAFETY: as the caller says.
    let attributes: IMFAttributes = unsafe { take((api.create_attributes)(&mut raw, 1), raw) }?;
    // SAFETY: the key and the value are references to constants.
    unsafe { attributes.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4) }?;
    let mut raw = ptr::null_mut();
    // SAFETY: as the caller says; the pointers are those of interfaces that are alive.
    unsafe {
        take(
            (api.create_sink_writer)(
                PCWSTR::null(),
                byte_stream.as_raw(),
                attributes.as_raw(),
                &mut raw,
            ),
            raw,
        )
    }
}

/// The type of the audio the file holds: AAC-LC, mono, 48 kHz. The encoder takes the sample rates 44.1 and 48 kHz and the
/// bit rates 96, 128, 160 and 192 kbps.
///
/// # Safety
/// Media Foundation has to be started.
unsafe fn file_type(api: &Api) -> windows::core::Result<IMFMediaType> {
    let mut raw = ptr::null_mut();
    // SAFETY: as the caller says.
    let media_type: IMFMediaType = unsafe { take((api.create_media_type)(&mut raw), raw) }?;
    // SAFETY: the keys and the values are references to constants.
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        media_type.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, M4A_RATE)?;
        media_type.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 1)?;
        media_type.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        media_type.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, M4A_BITRATE / 8)?;
        media_type.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 1)?;
        // Raw AAC, which is what an MP4 file holds.
        media_type.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
        media_type.SetUINT32(
            &MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION,
            AAC_PROFILE_LEVEL_L2,
        )?;
    }
    Ok(media_type)
}

/// The type of the blocks the sink is given: 16-bit PCM, mono, 48 kHz.
///
/// # Safety
/// Media Foundation has to be started.
unsafe fn block_type(api: &Api) -> windows::core::Result<IMFMediaType> {
    let mut raw = ptr::null_mut();
    // SAFETY: as the caller says.
    let media_type: IMFMediaType = unsafe { take((api.create_media_type)(&mut raw), raw) }?;
    // SAFETY: the keys and the values are references to constants.
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        media_type.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, M4A_RATE)?;
        media_type.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 1)?;
        media_type.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        media_type.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 2)?;
        media_type.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, M4A_RATE * 2)?;
    }
    Ok(media_type)
}

/// Gives the writer its two types and starts it. Returns the number of the stream, and on failure what was being done.
///
/// # Safety
/// Media Foundation has to be started.
unsafe fn begin(
    api: &Api,
    writer: &IMFSinkWriter,
) -> Result<u32, (&'static str, windows::core::Error)> {
    // SAFETY: as the caller says.
    let (file_type, block_type) = unsafe {
        (
            file_type(api).map_err(|e| ("making the type of the file", e))?,
            block_type(api).map_err(|e| ("making the type of the blocks", e))?,
        )
    };
    // SAFETY: the types are made, and the writer is not started yet.
    unsafe {
        let stream = writer
            .AddStream(&file_type)
            .map_err(|e| ("adding the audio to the file", e))?;
        // This is where Windows looks for the AAC encoder, and where it is found missing.
        writer
            .SetInputMediaType(stream, &block_type, None::<&IMFAttributes>)
            .map_err(|e| ("finding the AAC encoder", e))?;
        writer
            .BeginWriting()
            .map_err(|e| ("starting to write", e))?;
        Ok(stream)
    }
}

/// A file being written by the Sink Writer.
struct MfSink {
    /// `None` once it is finished or dropped.
    writer: Option<IMFSinkWriter>,
    byte_stream: Option<IMFByteStream>,
    api: &'static Api,
    /// The number of the stream of the file that the audio is written to.
    stream: u32,
    /// How many samples have been written, which is where the next block is in time.
    frames: u64,
    out: PathBuf,
    part: PathBuf,
    /// The `.part` file has become `out`.
    published: bool,
    // Dropped after the writer (`drop` releases it), in this order: Media Foundation, then COM.
    _startup: Startup,
    _com: Com,
}

// SAFETY: Media Foundation's objects are free-threaded (they live in the multithreaded apartment), the sink is only ever
// used through `&mut self`, and the COM guard puts COM back only on the thread that started it.
unsafe impl Send for MfSink {}

impl MfSink {
    /// Writes one piece: converts it to 16-bit PCM in a buffer of Media Foundation's, and hands it to the writer.
    fn push(&mut self, piece: &[f32]) -> Result<(), (&'static str, windows::core::Error)> {
        let writer = self
            .writer
            .as_ref()
            .ok_or(("writing", windows::core::Error::from(E_FAIL)))?;
        let bytes = u32::try_from(piece.len() * 2).expect("a piece is a second at most");
        let api = self.api;
        // SAFETY: Media Foundation is started (the sink holds `_startup`). The buffer is locked while it is written
        // to, and is as long as the piece (two bytes a sample).
        unsafe {
            let mut raw = ptr::null_mut();
            let buffer: IMFMediaBuffer = take((api.create_memory_buffer)(bytes, &mut raw), raw)
                .map_err(|e| ("making a buffer", e))?;
            let mut data: *mut u8 = ptr::null_mut();
            buffer
                .Lock(&mut data, None, None)
                .map_err(|e| ("filling a buffer", e))?;
            for (i, sample) in piece.iter().enumerate() {
                let pcm = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
                data.cast::<i16>().add(i).write_unaligned(pcm);
            }
            buffer.Unlock().map_err(|e| ("filling a buffer", e))?;
            buffer
                .SetCurrentLength(bytes)
                .map_err(|e| ("filling a buffer", e))?;

            let mut raw = ptr::null_mut();
            let sample: IMFSample =
                take((api.create_sample)(&mut raw), raw).map_err(|e| ("making a sample", e))?;
            let start = self.frames * TICKS_PER_SECOND / u64::from(M4A_RATE);
            let end = (self.frames + piece.len() as u64) * TICKS_PER_SECOND / u64::from(M4A_RATE);
            sample
                .AddBuffer(&buffer)
                .and_then(|()| sample.SetSampleTime(start as i64))
                .and_then(|()| sample.SetSampleDuration((end - start) as i64))
                .map_err(|e| ("making a sample", e))?;
            writer
                .WriteSample(self.stream, &sample)
                .map_err(|e| ("encoding", e))?;
        }
        self.frames += piece.len() as u64;
        Ok(())
    }

    /// Lets go of the writer and the file, so that the file can be renamed or removed.
    fn release(&mut self) {
        self.writer = None;
        if let Some(byte_stream) = self.byte_stream.take() {
            // SAFETY: the byte stream is open. It may be closed already, by the writer.
            let _ = unsafe { byte_stream.Close() };
        }
    }
}

impl M4aSink for MfSink {
    fn write(&mut self, mono_48k: &[f32]) -> io::Result<()> {
        for piece in mono_48k.chunks(PIECE_SAMPLES) {
            self.push(piece).map_err(|(what, e)| {
                io::Error::other(format!("Media Foundation failed {what}: {e}"))
            })?;
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> io::Result<()> {
        let Some(writer) = self.writer.as_ref() else {
            return Err(io::Error::other("the file is already closed"));
        };
        // SAFETY: the writer is started and has been given all the audio. This writes what the encoder holds and the
        // index of the MP4 file, and waits for it.
        let finalized = unsafe { writer.Finalize() };
        self.release();
        finalized.map_err(|e| {
            io::Error::other(format!("Media Foundation failed finishing the file: {e}"))
        })?;
        publish(&self.part, &self.out)?;
        self.published = true;
        Ok(())
    }
}

impl Drop for MfSink {
    /// A sink that is dropped without being finished leaves no file.
    fn drop(&mut self) {
        self.release();
        if !self.published {
            remove_part(&self.part);
        }
    }
}

/// COM, started on the thread that makes the sink (Media Foundation needs it).
struct Com {
    thread: ThreadId,
    /// COM was started by this guard, which has to put it back.
    started: bool,
}

impl Com {
    fn start() -> windows::core::Result<Com> {
        // SAFETY: nothing is passed in.
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result == RPC_E_CHANGED_MODE {
            // The thread has COM in another mode (a thread of a window): it can be used, and is not ours to put back.
            return Ok(Com {
                thread: thread::current().id(),
                started: false,
            });
        }
        // Started, or already started the same way on this thread: either way it is put back once.
        result.ok()?;
        Ok(Com {
            thread: thread::current().id(),
            started: true,
        })
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // COM is put back on the thread that started it. A sink that is dropped on another thread leaves it: the thread
        // that started it is the pipeline's blocking one, which stays in COM until it ends, and that is harmless.
        if self.started && thread::current().id() == self.thread {
            // SAFETY: balances the `CoInitializeEx` of `start`, on the same thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// Media Foundation, started.
struct Startup(&'static Api);

impl Startup {
    fn new(api: &'static Api) -> windows::core::Result<Startup> {
        // SAFETY: the function is Media Foundation's own.
        unsafe { (api.startup)(MF_VERSION, MFSTARTUP_LITE) }.ok()?;
        Ok(Startup(api))
    }
}

impl Drop for Startup {
    fn drop(&mut self) {
        // SAFETY: balances the `MFStartup` of `new`.
        let _ = unsafe { (self.0.shutdown)() };
    }
}

/// An interface that a Media Foundation function has made, taken from the pointer it filled in.
///
/// # Safety
/// `raw` is what the function that returned `result` has filled in: a pointer to a new `T`, when it succeeded.
unsafe fn take<T: Interface>(result: HRESULT, raw: *mut c_void) -> windows::core::Result<T> {
    result.ok()?;
    // SAFETY: as the caller says.
    Ok(unsafe { T::from_raw(raw) })
}

/// The functions of Media Foundation that are not methods of an interface, found in its DLLs.
struct Api {
    startup: unsafe extern "system" fn(u32, u32) -> HRESULT,
    shutdown: unsafe extern "system" fn() -> HRESULT,
    create_attributes: unsafe extern "system" fn(*mut *mut c_void, u32) -> HRESULT,
    create_media_type: unsafe extern "system" fn(*mut *mut c_void) -> HRESULT,
    create_memory_buffer: unsafe extern "system" fn(u32, *mut *mut c_void) -> HRESULT,
    create_sample: unsafe extern "system" fn(*mut *mut c_void) -> HRESULT,
    create_file: unsafe extern "system" fn(i32, i32, i32, PCWSTR, *mut *mut c_void) -> HRESULT,
    create_sink_writer:
        unsafe extern "system" fn(PCWSTR, *mut c_void, *mut c_void, *mut *mut c_void) -> HRESULT,
}

impl Api {
    /// The functions, if this Windows has Media Foundation. The DLLs are looked for once, and kept loaded.
    fn get() -> Option<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(Api::load).as_ref()
    }

    fn load() -> Option<Api> {
        // Only from the system folder: never a DLL of the same name next to the program or in the current folder.
        let load = |name: PCWSTR| {
            // SAFETY: the name is a constant string.
            unsafe { LoadLibraryExW(name, None, LOAD_LIBRARY_SEARCH_SYSTEM32) }.ok()
        };
        let platform = load(w!("mfplat.dll"))?;
        let read_write = load(w!("mfreadwrite.dll"))?;
        // SAFETY: each name is that of a function of Media Foundation, whose signature is the type it is given here.
        unsafe {
            Some(Api {
                startup: symbol(platform, s!("MFStartup"))?,
                shutdown: symbol(platform, s!("MFShutdown"))?,
                create_attributes: symbol(platform, s!("MFCreateAttributes"))?,
                create_media_type: symbol(platform, s!("MFCreateMediaType"))?,
                create_memory_buffer: symbol(platform, s!("MFCreateMemoryBuffer"))?,
                create_sample: symbol(platform, s!("MFCreateSample"))?,
                create_file: symbol(platform, s!("MFCreateFile"))?,
                create_sink_writer: symbol(read_write, s!("MFCreateSinkWriterFromURL"))?,
            })
        }
    }
}

/// A function of a loaded DLL, as the pointer type `T`.
///
/// # Safety
/// `T` is the type of a function pointer with the signature of the function that `name` names.
unsafe fn symbol<T: Copy>(module: HMODULE, name: PCSTR) -> Option<T> {
    // SAFETY: as the caller says.
    let address = unsafe { GetProcAddress(module, name) }?;
    debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of_val(&address));
    // SAFETY: both are pointers to a function.
    Some(unsafe { std::mem::transmute_copy(&address) })
}

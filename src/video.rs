//! What an uploaded clip has to be before it's shared: an MP4 a browser
//! plays. Its index (the `moov` box) is read from the bucket, not the whole
//! file, and has to hold an H.264 video track with its decoder settings and
//! its frames, sound only as AAC, and every frame inside the file. The app
//! sends exactly that (its video copied as it is, the moov first); anything
//! else is refused, so the bucket only ever serves videos.

use std::future::Future;

/// How many of a file's top-level boxes are looked through for its index.
const MAX_BOXES: usize = 32;
/// The biggest index read: a 100 MB clip's is well under a megabyte.
const MAX_MOOV: u64 = 32 * 1024 * 1024;

/// What a clip's index says of it.
#[derive(Debug, PartialEq)]
pub struct Movie {
    pub width: u32,
    pub height: u32,
    pub duration_ms: i64,
    pub sound: bool,
}

/// Why a file isn't shared.
#[derive(Debug, PartialEq)]
pub enum Refused<E> {
    /// It couldn't be read (the bucket's error): not the file's fault.
    Read(E),
    /// It isn't a video a browser plays, and why.
    Not(String),
}

/// Whether the file of `size` bytes that `read(from, len)` reads is a video
/// a browser plays (see the module): what it is, or why it's refused.
pub async fn check<E, F, Fut>(size: u64, mut read: F) -> Result<Movie, Refused<E>>
where
    F: FnMut(u64, u64) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, E>>,
{
    let not = |why: &str| Refused::Not(why.to_owned());
    let mut at = 0;
    for i in 0..MAX_BOXES {
        if at + 8 > size {
            break;
        }
        let head = read(at, 16.min(size - at)).await.map_err(Refused::Read)?;
        let (len, kind, header) = box_header(&head, size - at).ok_or_else(|| not("its boxes don't add up"))?;
        if i == 0 && &kind != b"ftyp" {
            return Err(not("it isn't an MP4"));
        }
        if &kind == b"moov" {
            if len > MAX_MOOV {
                return Err(not("its index is too big"));
            }
            let moov = read(at, len).await.map_err(Refused::Read)?;
            if moov.len() as u64 != len {
                return Err(not("its index is cut short"));
            }
            return movie(&moov[header..], size).map_err(Refused::Not);
        }
        at += len;
    }
    Err(not("it has no index"))
}

/// A box's length (all of it), kind, and how long its header is, from its
/// first bytes `b`; `left`: how much there is from it on (a length of 0
/// means all of that). `None` when it doesn't fit.
fn box_header(b: &[u8], left: u64) -> Option<(u64, [u8; 4], usize)> {
    let kind: [u8; 4] = b.get(4..8)?.try_into().ok()?;
    let (len, header) = match u32::from_be_bytes(b.get(0..4)?.try_into().ok()?) {
        0 => (left, 8),
        1 => (u64::from_be_bytes(b.get(8..16)?.try_into().ok()?), 16),
        n => (u64::from(n), 8),
    };
    (len >= header as u64 && len <= left).then_some((len, kind, header))
}

/// Boxes: each one's kind and insides.
type Boxes<'a> = Vec<([u8; 4], &'a [u8])>;

/// The boxes directly in `b` (a box's insides).
fn children(b: &[u8]) -> Result<Boxes<'_>, String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < b.len() {
        let (len, kind, header) = box_header(&b[at..], (b.len() - at) as u64).ok_or("its index is damaged")?;
        let end = at + len as usize;
        out.push((kind, &b[at + header..end]));
        at = end;
    }
    Ok(out)
}

/// The first box of `kind` in `b`, its insides.
fn child<'a>(b: &'a [u8], kind: &[u8; 4]) -> Result<&'a [u8], String> {
    children(b)?.into_iter().find(|(k, _)| k == kind).map(|(_, inside)| inside).ok_or_else(|| {
        let name = String::from_utf8_lossy(kind);
        format!("its index has no {name}")
    })
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, String> {
    Ok(u32::from_be_bytes(b.get(at..at + 4).ok_or("its index is cut short")?.try_into().expect("four bytes")))
}

fn u64_at(b: &[u8], at: usize) -> Result<u64, String> {
    Ok(u64::from_be_bytes(b.get(at..at + 8).ok_or("its index is cut short")?.try_into().expect("eight bytes")))
}

/// The movie its index (`moov`'s insides) describes, in a file of
/// `file_size` bytes.
fn movie(moov: &[u8], file_size: u64) -> Result<Movie, String> {
    // Its length: a version 0 header has 32-bit times, 1 64-bit.
    let mvhd = child(moov, b"mvhd")?;
    let (scale, length) = match mvhd.first() {
        Some(0) => (u32_at(mvhd, 12)?, u64::from(u32_at(mvhd, 16)?)),
        Some(1) => (u32_at(mvhd, 20)?, u64_at(mvhd, 24)?),
        _ => return Err("its movie header is of an unknown kind".into()),
    };
    if scale == 0 || length == 0 {
        return Err("it has no length".into());
    }
    let duration_ms = i64::try_from(u128::from(length) * 1000 / u128::from(scale)).map_err(|_| "it's too long")?;
    let mut video = None;
    let mut sound = false;
    for (kind, trak) in children(moov)? {
        if &kind != b"trak" {
            continue;
        }
        match track(trak, file_size)? {
            Track::Video { width, height } => {
                video.get_or_insert((width, height));
            }
            Track::Sound => sound = true,
            Track::Other => {}
        }
    }
    let (width, height) = video.ok_or("it has no video")?;
    Ok(Movie { width, height, duration_ms, sound })
}

enum Track {
    Video { width: u32, height: u32 },
    Sound,
    Other,
}

/// What a track (`trak`'s insides) is, checked: H.264 video with its
/// settings and frames, or AAC sound, all of it inside the file.
fn track(trak: &[u8], file_size: u64) -> Result<Track, String> {
    let mdia = child(trak, b"mdia")?;
    let handler = child(mdia, b"hdlr")?.get(8..12).ok_or("a track's handler is cut short")?;
    let stbl = child(child(mdia, b"minf")?, b"stbl")?;
    // Its first sample description: what its frames are.
    let stsd = child(stbl, b"stsd")?;
    let entries = stsd.get(8..).ok_or("a track has no description")?;
    let (len, codec, header) = box_header(entries, entries.len() as u64).ok_or("a track's description is damaged")?;
    let entry = &entries[header..len as usize];
    let track = match handler {
        b"vide" => {
            if !matches!(&codec, b"avc1" | b"avc3") {
                return Err(format!("its video is {}, not H.264", String::from_utf8_lossy(&codec)));
            }
            // H.264's decoder settings, after the 78 bytes a picture's
            // description starts with.
            child(entry.get(78..).ok_or("its video's description is cut short")?, b"avcC")?;
            let tkhd = child(trak, b"tkhd")?;
            // Width and height in 16.16, after the times (32 or 64 bits).
            let at = match tkhd.first() {
                Some(0) => 76,
                Some(1) => 88,
                _ => return Err("a track header is of an unknown kind".into()),
            };
            Track::Video { width: u32_at(tkhd, at)? >> 16, height: u32_at(tkhd, at + 4)? >> 16 }
        }
        b"soun" if &codec == b"mp4a" => Track::Sound,
        b"soun" => return Err(format!("its sound is {}, not AAC", String::from_utf8_lossy(&codec))),
        _ => Track::Other,
    };
    let frames = frames(stbl, file_size)?;
    if matches!(track, Track::Video { .. }) && frames == 0 {
        return Err("its video has no frames".into());
    }
    Ok(track)
}

/// How many frames (samples) a track's table (`stbl`'s insides) has, all of
/// them inside the file: where its chunks start, and their sizes together.
fn frames(stbl: &[u8], file_size: u64) -> Result<u64, String> {
    let stsz = child(stbl, b"stsz")?;
    let (each, count) = (u64::from(u32_at(stsz, 4)?), u64::from(u32_at(stsz, 8)?));
    let total = if each == 0 {
        let (sizes, _) = stsz.get(12..).ok_or("a track's sizes are cut short")?.as_chunks::<4>();
        let sizes = sizes.get(..count as usize).ok_or("a track's sizes are cut short")?;
        sizes.iter().map(|s| u64::from(u32::from_be_bytes(*s))).sum()
    } else {
        each * count
    };
    if total > file_size {
        return Err("a track's frames take more than the file".into());
    }
    // Where its chunks start: 32-bit offsets, or 64-bit ones.
    let cut = "a track's chunks are cut short";
    let starts: Vec<u64> = match child(stbl, b"stco") {
        Ok(stco) => {
            let (o, _) = stco.get(8..).ok_or(cut)?.as_chunks::<4>();
            o.get(..u32_at(stco, 4)? as usize).ok_or(cut)?.iter().map(|o| u64::from(u32::from_be_bytes(*o))).collect()
        }
        Err(_) => {
            let co64 = child(stbl, b"co64")?;
            let (o, _) = co64.get(8..).ok_or(cut)?.as_chunks::<8>();
            o.get(..u32_at(co64, 4)? as usize).ok_or(cut)?.iter().map(|o| u64::from_be_bytes(*o)).collect()
        }
    };
    if starts.iter().any(|&o| o >= file_size) {
        return Err("a track's frames are outside the file".into());
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clip as the app uploads it: H.264 and AAC in an MP4, the index
    /// first (96×64, a second).
    const CLIP: &[u8] = include_bytes!("testdata/clip.mp4");
    /// The same with no sound.
    const SILENT: &[u8] = include_bytes!("testdata/silent.mp4");

    /// Check `file` as the server does, read from memory: why it's refused.
    fn checked(file: &[u8]) -> Result<Movie, String> {
        let read = |from: u64, len: u64| {
            let (from, len) = (from as usize, len as usize);
            std::future::ready(Ok::<_, ()>(file[from.min(file.len())..(from + len).min(file.len())].to_vec()))
        };
        let checked =
            tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(check(file.len() as u64, read));
        checked.map_err(|r| match r {
            Refused::Not(why) => why,
            Refused::Read(()) => unreachable!("memory reads"),
        })
    }

    /// Files of your own, as the server would take them (the app's upload
    /// copy of a clip, say): `VIDEO=<path>[;<path>…] cargo test any_file --
    /// --ignored --nocapture`.
    #[test]
    #[ignore = "checks the files VIDEO names"]
    fn any_file() {
        let paths = std::env::var_os("VIDEO").expect("VIDEO=<path>");
        for path in std::env::split_paths(&paths) {
            let checked = checked(&std::fs::read(&path).unwrap());
            println!("{}: {checked:?}", path.display());
            assert!(checked.is_ok(), "{}", path.display());
        }
    }

    /// A file that can't be read isn't refused for what it is.
    #[test]
    fn a_failed_read_is_the_buckets() {
        let read = |_: u64, _: u64| std::future::ready(Err::<Vec<u8>, _>("the bucket is away"));
        let checked =
            tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(check(CLIP.len() as u64, read));
        assert_eq!(checked, Err(Refused::Read("the bucket is away")));
    }

    /// `file` with the first `from` in its index made `to` (as long): the
    /// file type at the start names brands like avc1 too.
    fn swapped(file: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
        let &(moov, len, _) = boxes(file).iter().find(|b| &b.2 == b"moov").unwrap();
        let mut out = file.to_vec();
        let at = moov + out[moov..moov + len].windows(from.len()).position(|w| w == from).expect("it's there");
        out[at..at + to.len()].copy_from_slice(to);
        out
    }

    /// The top-level boxes of `file`: where each starts, and how long it is.
    fn boxes(file: &[u8]) -> Vec<(usize, usize, [u8; 4])> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < file.len() {
            let (len, kind, _) = box_header(&file[at..], (file.len() - at) as u64).unwrap();
            out.push((at, len as usize, kind));
            at += len as usize;
        }
        out
    }

    #[test]
    fn a_clip_from_the_app_plays() {
        let movie = checked(CLIP).unwrap();
        assert_eq!((movie.width, movie.height, movie.sound), (96, 64, true));
        // A second, give or take the sound's first frame.
        assert!((950..=1100).contains(&movie.duration_ms), "{movie:?}");
        assert_eq!(checked(SILENT).map(|m| m.sound), Ok(false));
    }

    /// With the index at the end (not the app's, but still an MP4 that
    /// plays), it's found past the frames.
    #[test]
    fn the_index_is_found_anywhere() {
        let boxes = boxes(CLIP);
        let mut moved = Vec::new();
        for kind in [b"ftyp", b"mdat", b"moov"] {
            let &(at, len, _) = boxes.iter().find(|b| &b.2 == kind).unwrap();
            moved.extend_from_slice(&CLIP[at..at + len]);
        }
        assert_eq!(checked(&moved).map(|m| m.width), Ok(96));
    }

    #[test]
    fn what_isnt_a_playable_video_is_refused() {
        // Not an MP4 at all, or only its first box.
        assert!(checked(b"PK\x03\x04 a zip, not a video, but long enough").is_err());
        let ftyp = boxes(CLIP)[0];
        assert_eq!(checked(&CLIP[..ftyp.1]), Err("it has no index".into()));
        // A first box that's an MP4's, then anything.
        let mut fake = CLIP[..ftyp.1].to_vec();
        fake.extend_from_slice(&[7; 4096]);
        assert!(checked(&fake).is_err());
        // Video that isn't H.264, or H.264 without its settings.
        assert_eq!(checked(&swapped(CLIP, b"avc1", b"hvc1")), Err("its video is hvc1, not H.264".into()));
        assert_eq!(checked(&swapped(CLIP, b"avcC", b"xxxx")), Err("its index has no avcC".into()));
        // Sound that isn't AAC.
        assert_eq!(checked(&swapped(CLIP, b"mp4a", b"Opus")), Err("its sound is Opus, not AAC".into()));
        // No video track: its handler renamed.
        assert_eq!(checked(&swapped(CLIP, b"vide", b"text")), Err("it has no video".into()));
        // Frames outside the file: it's cut short.
        let cut = &CLIP[..CLIP.len() - 2000];
        assert!(checked(cut).is_err());
    }
}

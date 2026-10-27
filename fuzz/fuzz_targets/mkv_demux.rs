//! cargo +nightly fuzz run mkv_demux -- -max_len=1048576
//! Seed with: corpus/generated and corpus/synthetic.
#![no_main]

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use plaid_core::{Demuxer, ErrorClass, Limits};
use plaid_demux::mkv::MkvDemuxer;

fuzz_target!(|data: &[u8]| {
    // Small limits so the fuzzer explores logic rather than allocation.
    let limits = Limits { max_element_size: 1 << 20, max_frame_size: 4 << 20, max_attachment_size: 1 << 20, max_streams: 64 };
    let Ok(mut demuxer) = MkvDemuxer::with_limits(Cursor::new(data.to_vec()), limits) else { return };
    // Every call consumes input, so this bound only trips on a real hang.
    for _ in 0..data.len() * 4 + 64 {
        match demuxer.read_packet() {
            Ok(Some(_)) => {}
            Ok(None) => return,
            Err(e) if matches!(e.class(), ErrorClass::Corrupt | ErrorClass::Resource) => {}
            Err(_) => return,
        }
    }
    panic!("demuxer did not terminate");
});

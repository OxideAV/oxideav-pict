#![no_main]
//! Decode-free walkers: the contract `probe` (bool sniff) and `info`
//! (header only), the depth `inspect` walker, plus the typed QuickTime
//! payload parsers on arbitrary bytes.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let probed = oxideav_pict::probe(data);
    let info = oxideav_pict::info(data);
    let inspected = oxideav_pict::inspect(data);
    if !probed {
        assert!(info.is_err());
        assert!(inspected.is_err());
    }
    if let (Ok(i), Ok(p)) = (&info, &inspected) {
        assert_eq!((i.width, i.height), (p.width, p.height));
        assert_eq!(i.has_launch_stub, p.has_launch_stub);
        assert_eq!(i.header, p.header);
    }
    let _ = oxideav_pict::parse_compressed_quicktime(data);
    let _ = oxideav_pict::parse_uncompressed_quicktime(data);
});

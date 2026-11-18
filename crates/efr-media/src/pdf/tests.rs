use miniz_oxide::deflate::compress_to_vec_zlib;
use proptest::prelude::*;

use super::*;

/// A PDF of a browser print: plain objects, a classic cross-reference table and a
/// trailer, with spaces between the names.
fn browser_print(pages: u32) -> Vec<u8> {
    format!(
        "%PDF-1.4\n%\u{e2}\u{e3}\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
         2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count {pages} >>\nendobj\n\
         3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << >> >> >>\nendobj\n\
         4 0 obj\n<< /Length 18 >>\nstream\n<< (not a dict) >>\nendstream\nendobj\n\
         xref\n0 5\n0000000000 65535 f \ntrailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n0\n%%EOF\n"
    )
    .into_bytes()
}

/// A PDF of an office tool: no spaces between the names, a page tree of two levels and
/// an outline with a count of its own.
fn office(pages: u32) -> Vec<u8> {
    let half = pages / 2;
    format!(
        "%PDF-1.7\n1 0 obj<</Type/Catalog/Pages 2 0 R/Outlines 9 0 R>>endobj\n\
         2 0 obj<</Type/Pages/Kids[3 0 R 4 0 R]/Count {pages}>>endobj\n\
         3 0 obj<</Type/Pages/Parent 2 0 R/Kids[]/Count {half}>>endobj\n\
         4 0 obj<</Type/Pages/Parent 2 0 R/Kids[]/Count {}>>endobj\n\
         9 0 obj<</Type/Outlines/Count 900>>endobj\n\
         trailer<</Size 10/Root 1 0 R/Info<</Title(a \\) (nested) title)>>>>\n%%EOF\n",
        pages - half
    )
    .into_bytes()
}

/// A PDF of pdfTeX: the page tree in a compressed object stream, a cross-reference
/// stream, and the length of the object stream in an indirect object when `indirect`.
fn pdftex(pages: u32, indirect: bool) -> Vec<u8> {
    let objects =
        format!("1 0 2 40 <</Type/Catalog/Pages 2 0 R>> <</Type/Pages/Kids[3 0 R]/Count {pages}>>");
    let packed = compress_to_vec_zlib(objects.as_bytes(), 6);
    let length = if indirect { "7 0 R".to_owned() } else { packed.len().to_string() };
    let mut out = format!(
        "%PDF-1.5\n5 0 obj\n<</Type/ObjStm/N 2/First 10/Length {length}/Filter/FlateDecode>>\nstream\n" // spellchecker:disable-line
    )
    .into_bytes();
    out.extend_from_slice(&packed);
    out.extend_from_slice(
        b"\nendstream\nendobj\n6 0 obj\n<</Type/XRef/Size 7/Root 1 0 R/W[1 2 1]/Length 0>>\nstream\n\nendstream\nendobj\n",
    );
    out
}

#[test]
fn a_browser_print_gives_its_page_count() {
    assert_eq!(pdf_facts(&browser_print(3)), PdfFacts { encrypted: false, pages: Some(3) });
}

#[test]
fn an_office_pdf_gives_the_count_of_its_root_and_not_of_its_outline() {
    assert_eq!(pdf_facts(&office(12)).pages, Some(12));
}

#[test]
fn a_pdftex_pdf_gives_the_count_in_its_object_stream() {
    assert_eq!(pdf_facts(&pdftex(140, false)).pages, Some(140));
    assert_eq!(pdf_facts(&pdftex(7, true)).pages, Some(7));
}

#[test]
fn an_encrypt_entry_in_a_trailer_or_a_cross_reference_stream_is_encrypted() {
    let trailer = String::from_utf8(browser_print(2))
        .unwrap()
        .replace("/Root 1 0 R >>", "/Root 1 0 R /Encrypt 8 0 R >>");
    assert!(pdf_facts(trailer.as_bytes()).encrypted);

    let mut stream = pdftex(2, false);
    let at = stream.windows(10).position(|w| w == b"/Type/XRef").unwrap();
    stream.splice(at..at, b"/Encrypt 8 0 R".iter().copied());
    assert!(pdf_facts(&stream).encrypted);
}

#[test]
fn an_encrypt_name_in_a_page_is_no_password() {
    let page = String::from_utf8(browser_print(2))
        .unwrap()
        .replace("/Type /Page ", "/Type /Page /Encrypt 1 ");
    assert!(!pdf_facts(page.as_bytes()).encrypted);
}

#[test]
fn a_pdf_with_no_readable_count_gives_none() {
    assert_eq!(pdf_facts(b"%PDF-1.4\n1 0 obj<</Type/Catalog>>endobj\n").pages, None);
    let mut broken = pdftex(9, false);
    let at = broken.windows(7).position(|w| w == b"stream\n").unwrap() + 7;
    broken[at..at + 8].fill(0);
    assert_eq!(pdf_facts(&broken).pages, None);
}

#[test]
fn an_object_stream_over_the_inflate_limit_gives_no_count() {
    let mut objects = vec![b' '; PDF_INFLATE_MAX + 1];
    objects.extend_from_slice(b"<</Type/Pages/Count 3>>");
    let packed = compress_to_vec_zlib(&objects, 1);
    let head = "%PDF-1.5\n<</Type/ObjStm/Length {}/Filter/FlateDecode>>stream\n"; // spellchecker:disable-line
    let mut pdf = head.replace("{}", &packed.len().to_string()).into_bytes();
    pdf.extend_from_slice(&packed);
    pdf.extend_from_slice(b"\nendstream\n<</Type/Pages/Count 2>>");
    assert_eq!(pdf_facts(&pdf).pages, None);
}

#[test]
fn a_dictionary_that_never_ends_stops_the_walk() {
    assert_eq!(pdf_facts(b"%PDF-1.4\n<</Type/Pages/Count 4 << ").pages, None);
}

proptest! {
    #[test]
    fn a_cut_or_changed_pdf_never_panics(
        which in 0_usize..3,
        cut in 0.0_f64..1.0,
        flips in proptest::collection::vec((0.0_f64..1.0, any::<u8>()), 0..8),
    ) {
        let mut bytes = match which {
            0 => browser_print(3),
            1 => office(5),
            _ => pdftex(4, false),
        };
        for (at, byte) in flips {
            let at = (at * bytes.len() as f64) as usize;
            if let Some(slot) = bytes.get_mut(at) {
                *slot = byte;
            }
        }
        let keep = (cut * bytes.len() as f64) as usize;
        let _ = pdf_facts(&bytes[..keep]);
    }
}

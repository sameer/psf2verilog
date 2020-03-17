use std::char::decode_utf16;
use std::convert::{TryFrom, TryInto};
use std::env;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::iter::Iterator;
use std::path::Path;

fn main() -> Result<(), ParseError> {
    match env::args().skip(1).next() {
        Some(path) => {
            let psf_file = File::open(Path::new(&path))?;
            PSF::try_from(psf_file)?.into_verilog();
        }
        None => {
            eprintln!("Usage: psf2verilog <PSF_FONT_FILENAME>");
        }
    }
    Ok(())
}

#[derive(Debug)]
enum ParseError {
    IoError(std::io::Error),
    NotPSF,
    UnsupportedVersion,
}

impl From<std::io::Error> for ParseError {
    fn from(err: std::io::Error) -> Self {
        Self::IoError(err)
    }
}

#[derive(Default, Debug)]
struct TableEntry {
    represented: Vec<char>,
    sequences: Vec<char>,
}

#[derive(Debug)]
enum Version {
    PSF1,
    PSF2,
}

#[derive(Debug)]
struct PSF {
    version: Version,
    charsize: u32,
    height: u32,
    width: u32,
    bitmap: Vec<u8>,
    table: Option<Vec<TableEntry>>,
}

impl PSF {
    const PSF1_MAGIC: [u8; 2] = [0x36, 0x04];
    const PSF1_MODE512: u8 = 0x01;
    const PSF1_MODEHASTAB: u8 = 0x02;
    const PSF1_MODEHASSEQ: u8 = 0x04;
    const PSF1_MAXMODE: u8 = 0x05;
    const PSF1_SEPARATOR: u16 = 0xFFFF;
    const PSF1_STARTSEQ: u16 = 0xFFFE;

    const PSF2_MAGIC: [u8; 4] = [0x72, 0xb5, 0x4a, 0x86];
    const PSF2_SEPARATOR: u8 = 0xFF;
    const PSF2_STARTSEQ: u8 = 0xFE;
    const PSF2_MAXVERSION: u32 = 0;
    const PSF2_HASUNICODETABLE: u32 = 0x01;

    fn parse_table<'a>(table: &'a [u8], version: &Version) -> Vec<TableEntry> {
        let mut entries = vec![];
        let mut current_entry = TableEntry::default();
        let mut sequence_started = false;
        match version {
            Version::PSF1 => {
                let mut codepoints = vec![];
                for i in (0..table.len()).step_by(2) {
                    let codepoint = u16::from_le_bytes(table[i..=i + 1].try_into().unwrap());
                    if codepoint == Self::PSF1_SEPARATOR || codepoint == Self::PSF1_STARTSEQ {
                        let mut chars = decode_utf16(codepoints).map(Result::unwrap).collect();
                        if !sequence_started {
                            current_entry.represented = chars;
                        } else {
                            current_entry.sequences.append(&mut chars);
                        }
                        codepoints = vec![];

                        if codepoint == Self::PSF1_SEPARATOR {
                            sequence_started = false;
                            entries.push(current_entry);
                            current_entry = TableEntry::default();
                        } else {
                            sequence_started = true;
                        }
                    } else {
                        codepoints.push(codepoint);
                    }
                }
            }
            Version::PSF2 => {
                let mut codepoints = vec![];
                for i in 0..table.len() {
                    let codepoint = table[i];
                    if codepoint == Self::PSF2_SEPARATOR || codepoint == Self::PSF2_STARTSEQ {
                        let mut chars = std::str::from_utf8(&codepoints).unwrap().chars().collect();
                        if !sequence_started {
                            current_entry.represented = chars;
                        } else {
                            current_entry.sequences.append(&mut chars);
                        }
                        codepoints = vec![];

                        if codepoint == Self::PSF2_SEPARATOR {
                            sequence_started = false;
                            entries.push(current_entry);
                            current_entry = TableEntry::default();
                        } else {
                            sequence_started = true;
                        }
                    } else {
                        codepoints.push(codepoint);
                    }
                }
            }
        }
        entries
    }

    fn into_verilog(&self) {
        let length = self.bitmap.len() as u32 / self.charsize;
        let input_width = (length as f64).log2().ceil() as u8;
        let output_width = self.charsize * 8;
        println!("module charactermap ( input wire clk, input wire [{}:0] character, output reg [{}:0] characterraster );", input_width - 1, output_width - 1);
        println!("always @(posedge clk) begin case (character)");
        for i in 0..length as usize {
            let mut s = String::with_capacity(output_width as usize);
            for j in 0..(self.charsize as usize) {
                s.push_str(&format!(
                    "{:0>2X}",
                    self.bitmap[i * self.charsize as usize + j]
                ));
            }
            println!(
                "    {}'b{:0>input_width$b} : characterraster = {}'h{};",
                input_width,
                i,
                output_width,
                s,
                input_width = input_width as usize
            );
        }
        println!("    default : characterraster = 0;");
        println!("endcase end");
        println!("endmodule");
    }
}

impl TryFrom<File> for PSF {
    type Error = ParseError;
    fn try_from(mut psf_file: File) -> Result<Self, Self::Error> {
        let mut magic = [0u8; 4];
        psf_file.read_exact(&mut magic)?;
        if magic[0..2] == Self::PSF1_MAGIC {
            let version = Version::PSF1;
            let mode = magic[2];
            let height = magic[3];
            let width = 8;
            let length = if mode & Self::PSF1_MODE512 != 0 {
                512
            } else {
                256
            };
            let charsize = height as usize;
            let mut bitmap = vec![0u8; charsize * length];
            psf_file.read_exact(&mut bitmap)?;
            let mut table_buf = vec![];
            let table = if mode & Self::PSF1_MODEHASTAB != 0 {
                psf_file.read_to_end(&mut table_buf)?;
                Some(Self::parse_table(&table_buf, &version))
            } else {
                None
            };
            Ok(PSF {
                version,
                charsize: charsize as u32,
                height: height as u32,
                width,
                bitmap,
                table,
            })
        } else if magic == Self::PSF2_MAGIC {
            let version = Version::PSF2;
            let mut rest_of_header = [0u8; 7 * 4];
            psf_file.read_exact(&mut rest_of_header)?;
            let header_version = u32::from_le_bytes(rest_of_header[0..4].try_into().unwrap());
            let header_size = u32::from_le_bytes(rest_of_header[4..8].try_into().unwrap());
            let flags = u32::from_le_bytes(rest_of_header[8..12].try_into().unwrap());
            let length = u32::from_le_bytes(rest_of_header[12..16].try_into().unwrap());
            let charsize = u32::from_le_bytes(rest_of_header[16..20].try_into().unwrap());
            let height = u32::from_le_bytes(rest_of_header[20..24].try_into().unwrap());
            let width = u32::from_le_bytes(rest_of_header[24..28].try_into().unwrap());
            if header_size >= 32 {
                // Skip the remainder of the header
                psf_file.seek(SeekFrom::Current((header_size - 32) as i64))?;
            } else {
                eprintln!("header_size should be >= 32 but = {}", header_size);
            }
            let mut bitmap = vec![0u8; (charsize * length) as usize];
            psf_file.read_exact(&mut bitmap)?;

            let table = if flags & Self::PSF2_HASUNICODETABLE > 0 {
                let mut table_buf = vec![];
                psf_file.read_to_end(&mut table_buf)?;
                Some(Self::parse_table(&mut table_buf, &version))
            } else {
                None
            };

            if header_version > Self::PSF2_MAXVERSION {
                Err(ParseError::UnsupportedVersion)
            } else {
                Ok(PSF {
                    version,
                    charsize,
                    height,
                    width,
                    bitmap,
                    table,
                })
            }
        } else {
            Err(ParseError::NotPSF)
        }
    }
}

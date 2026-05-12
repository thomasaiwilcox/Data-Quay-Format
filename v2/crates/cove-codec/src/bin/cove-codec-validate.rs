use std::{env, fs, process};

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: cove-codec-validate <codec-descriptor-or-envelope-section.bin>");
        process::exit(2);
    };
    if args.next().is_some() {
        eprintln!("usage: cove-codec-validate <codec-descriptor-or-envelope-section.bin>");
        process::exit(2);
    }
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{path}: {error}");
            process::exit(1);
        }
    };
    match cove_codec::CodecExtensionDescriptorV2::parse_many(&bytes) {
        Ok(descriptors) => {
            println!(
                "valid COVE-CX descriptor section: {} descriptors",
                descriptors.len()
            );
            for descriptor in descriptors {
                println!(
                    "codec_id={} {}::{} v{}.{} requirement={:?} status={:?}",
                    descriptor.codec_id,
                    descriptor.namespace,
                    descriptor.name,
                    descriptor.version_major,
                    descriptor.version_minor,
                    descriptor.requirement,
                    descriptor.specification_status
                );
            }
        }
        Err(descriptor_error) => {
            match cove_codec::RegisteredEncodingEnvelopeV2::parse_many(&bytes) {
                Ok(envelopes) => {
                    println!(
                        "valid COVE-CX registered encoding envelope section: {} envelopes",
                        envelopes.len()
                    );
                    for envelope in envelopes {
                        println!(
                            "codec_id={} v{}.{} logical_len={} encoded_bytes={} fallback_bytes={}",
                            envelope.codec_id,
                            envelope.codec_version_major,
                            envelope.codec_version_minor,
                            envelope.logical_len,
                            envelope.encoded_payload_length,
                            envelope.fallback_payload_length
                        );
                    }
                }
                Err(envelope_error) => {
                    eprintln!(
                        "{path}: not a valid COVE-CX descriptor or registered encoding envelope section: descriptor={descriptor_error}; envelope={envelope_error}"
                    );
                    process::exit(1);
                }
            }
        }
    }
}

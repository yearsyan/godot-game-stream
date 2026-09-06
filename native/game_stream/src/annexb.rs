// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

//! Annex-B NAL helpers for H.264 and HEVC parameter-set caching.

pub const H264_SPS: u8 = 7;
pub const H264_PPS: u8 = 8;
pub const HEVC_VPS: u8 = 32;
pub const HEVC_SPS: u8 = 33;
pub const HEVC_PPS: u8 = 34;

pub fn iter_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut index = 0;
    while index + 3 < data.len() {
        if data[index] == 0 && data[index + 1] == 0 {
            if data[index + 2] == 1 {
                starts.push(index);
                index += 3;
                continue;
            }
            if data[index + 2] == 0 && data[index + 3] == 1 {
                starts.push(index);
                index += 4;
                continue;
            }
        }
        index += 1;
    }

    starts
        .iter()
        .enumerate()
        .filter_map(|(index, start)| {
            let end = starts.get(index + 1).copied().unwrap_or(data.len());
            (end > *start).then_some(&data[*start..end])
        })
        .collect()
}

pub fn h264_nal_type(nal: &[u8]) -> Option<u8> {
    nal_payload(nal).first().map(|byte| byte & 0x1f)
}

pub fn hevc_nal_type(nal: &[u8]) -> Option<u8> {
    nal_payload(nal).first().map(|byte| (byte >> 1) & 0x3f)
}

fn nal_payload(nal: &[u8]) -> &[u8] {
    if nal.starts_with(&[0, 0, 0, 1]) {
        &nal[4..]
    } else if nal.starts_with(&[0, 0, 1]) {
        &nal[3..]
    } else {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_three_and_four_byte_start_codes() {
        let data = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 4,
        ];
        let nals = iter_nals(&data);
        assert_eq!(nals.len(), 3);
        assert_eq!(h264_nal_type(nals[0]), Some(H264_SPS));
        assert_eq!(h264_nal_type(nals[1]), Some(H264_PPS));
        assert_eq!(h264_nal_type(nals[2]), Some(5));
    }

    #[test]
    fn parses_hevc_parameter_set_types() {
        let data = [
            0,
            0,
            0,
            1,
            HEVC_VPS << 1,
            1,
            0,
            0,
            1,
            HEVC_SPS << 1,
            1,
            0,
            0,
            0,
            1,
            HEVC_PPS << 1,
            1,
        ];
        let nals = iter_nals(&data);
        assert_eq!(hevc_nal_type(nals[0]), Some(HEVC_VPS));
        assert_eq!(hevc_nal_type(nals[1]), Some(HEVC_SPS));
        assert_eq!(hevc_nal_type(nals[2]), Some(HEVC_PPS));
    }
}

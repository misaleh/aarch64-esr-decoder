// Copyright 2021 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

mod abort;
mod common;
mod ldc;
mod mcr;
mod wf;

use abort::{decode_iss_data_abort, decode_iss_instruction_abort};
use bit_field::BitField;
use ldc::decode_iss_ldc;
use mcr::{decode_iss_mcr, decode_iss_mcrr};
use std::fmt::{self, Debug, Display, Formatter};
use std::num::ParseIntError;
use thiserror::Error;
use wf::decode_iss_wf;

/// Information about a particular field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldInfo {
    /// The short name of the field, e.g. "ISS".
    pub name: &'static str,
    /// The index of the lowest bit of the field.
    pub start: usize,
    /// The number of bits in the field.
    pub width: usize,
    /// The value of the field.
    pub value: u64,
    /// More information about the field and subfields, if available.
    pub decoded: Option<Decoded>,
}

impl FieldInfo {
    fn get(register: u64, name: &'static str, start: usize, end: usize) -> Self {
        let value = register.get_bits(start..end);
        Self {
            name,
            start,
            width: end - start,
            value,
            decoded: None,
        }
    }

    fn get_bit(register: u64, name: &'static str, bit: usize) -> Self {
        Self::get(register, name, bit, bit + 1)
    }

    fn with_decoded(self, decoded: Decoded) -> Self {
        Self {
            decoded: Some(decoded),
            ..self
        }
    }

    fn with_description(self, description: String) -> Self {
        self.with_decoded(Decoded {
            description: Some(description),
            fields: vec![],
        })
    }

    fn as_bit(&self) -> bool {
        assert!(self.width == 1);
        self.value == 1
    }

    /// Assuming this field has a width of exactly 1, describe it with the given function.
    ///
    /// Panics if `self.width != 1`.
    fn describe_bit<F>(self, describer: F) -> Self
    where
        F: FnOnce(bool) -> &'static str,
    {
        let bit = self.as_bit();
        let description = describer(bit).to_string();
        self.with_description(description)
    }

    fn describe<F>(self, describer: F) -> Result<Self, DecodeError>
    where
        F: FnOnce(u64) -> Result<&'static str, DecodeError>,
    {
        let description = describer(self.value)?.to_string();
        Ok(self.with_description(description))
    }

    fn check_res0(self) -> Result<Self, DecodeError> {
        if self.value != 0 {
            Err(DecodeError::InvalidRes0 { res0: self.value })
        } else {
            Ok(self)
        }
    }

    /// Returns the value as a hexadecimal string, or "true" or "false" if it is a single bit.
    pub fn value_string(&self) -> String {
        if self.width == 1 {
            if self.value == 1 { "true" } else { "false" }.to_string()
        } else {
            format!("{:#01$x}", self.value, (self.width + 3) / 4 + 2,)
        }
    }

    /// Returns the value as a binary strings.
    pub fn value_binary_string(&self) -> String {
        format!("{:#01$b}", self.value, self.width + 2)
    }
}

impl Display for FieldInfo {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        if self.width == 1 {
            write!(
                f,
                "{}: {}",
                self.name,
                if self.value == 1 { "true" } else { "false" }
            )
        } else {
            write!(
                f,
                "{}: {} {}",
                self.name,
                self.value_string(),
                self.value_binary_string(),
            )
        }
    }
}

/// An error decoding an ESR value.
#[derive(Debug, Error)]
pub enum DecodeError {
    /// A RES0 field was not 0.
    #[error("Invalid ESR, res0 is {res0:#x}")]
    InvalidRes0 { res0: u64 },
    /// The EC field had an invalid value.
    #[error("Invalid EC {ec:#x}")]
    InvalidEc { ec: u64 },
    /// The DFSC or IFSC field had an invalid value.
    #[error("Invalid DFSC or IFSC {fsc:#x}")]
    InvalidFsc { fsc: u64 },
    /// The SET field had an invalid value.
    #[error("Invalid SET {set:#x}")]
    InvalidSet { set: u64 },
    /// The AM field had an invalid value.
    #[error("Invalid AM {am:#x}")]
    InvalidAm { am: u64 },
}

/// Information about the decoding of a field (or the entire ESR value).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decoded {
    /// A description explaining the field value, if available.
    pub description: Option<String>,
    /// Any sub-fields.
    pub fields: Vec<FieldInfo>,
}

fn decode_iss_res0(iss: u64) -> Result<Decoded, DecodeError> {
    if iss == 0 {
        Ok(Decoded {
            description: Some("ISS is RES0".to_string()),
            fields: vec![],
        })
    } else {
        Err(DecodeError::InvalidRes0 { res0: iss })
    }
}

/// Decodes the given Exception Syndrome Register value, or returns an error if it is not valid.
pub fn decode(esr: u64) -> Result<Decoded, DecodeError> {
    let res0 = FieldInfo::get(esr, "RES0", 37, 64).check_res0()?;
    let iss2 = FieldInfo::get(esr, "ISS2", 32, 37);
    let ec = FieldInfo::get(esr, "EC", 26, 32);
    let il = FieldInfo::get_bit(esr, "IL", 25).describe_bit(describe_il);
    let iss = FieldInfo::get(esr, "ISS", 0, 25);
    let (class, iss_decoded) = match ec.value {
        0b000000 => ("Unknown reason", Some(decode_iss_res0(iss.value)?)),
        0b000001 => (
            "Wrapped WF* instruction execution",
            Some(decode_iss_wf(iss.value)?),
        ),
        0b000011 => (
            "Trapped MCR or MRC access with coproc=0b1111",
            Some(decode_iss_mcr(iss.value)?),
        ),
        0b000100 => (
            "Trapped MCRR or MRRC access with coproc=0b1111",
            Some(decode_iss_mcrr(iss.value)?),
        ),
        0b000101 => (
            "Trapped MCR or MRC access with coproc=0b1110",
            Some(decode_iss_mcr(iss.value)?),
        ),
        0b000110 => (
            "Trapped LDC or STC access",
            Some(decode_iss_ldc(iss.value)?),
        ),
        0b000111 => (
            "Trapped access to SVE, Advanced SIMD or floating point",
            None,
        ),
        0b001010 => (
            "Trapped execution of an LD64B, ST64B, ST64BV, or ST64BV0 instruction",
            None,
        ),
        0b001100 => ("Trapped MRRC access with (coproc==0b1110)", None),
        0b001101 => ("Branch Target Exception", None),
        0b001110 => ("Illegal Execution state", Some(decode_iss_res0(iss.value)?)),
        0b010001 => ("SVC instruction execution in AArch32 state", None),
        0b010101 => ("SVC instruction execution in AArch64 state", None),
        0b011000 => (
            "Trapped MSR, MRS or System instruction execution in AArch64 state",
            None,
        ),
        0b011001 => (
            "Access to SVE functionality trapped as a result of CPACR_EL1.ZEN, CPTR_EL2.ZEN, \
             CPTR_EL2.TZ, or CPTR_EL3.EZ",
            Some(decode_iss_res0(iss.value)?),
        ),
        0b011100 => (
            "Exception from a Pointer Authentication instruction authentication failure",
            None,
        ),
        0b100000 => (
            "Instruction Abort from a lower Exception level",
            Some(decode_iss_instruction_abort(iss.value)?),
        ),
        0b100001 => (
            "Instruction Abort taken without a change in Exception level",
            Some(decode_iss_instruction_abort(iss.value)?),
        ),
        0b100010 => (
            "PC alignment fault exception",
            Some(decode_iss_res0(iss.value)?),
        ),
        0b100100 => (
            "Data Abort from a lower Exception level",
            Some(decode_iss_data_abort(iss.value)?),
        ),
        0b100101 => (
            "Data Abort taken without a change in Exception level",
            Some(decode_iss_data_abort(iss.value)?),
        ),
        0b100110 => (
            "SP alignment fault exception",
            Some(decode_iss_res0(iss.value)?),
        ),
        0b101000 => (
            "Trapped floating-point exception taken from AArch32 state",
            None,
        ),
        0b101100 => (
            "Trapped floating-point exception taken from AArch64 state",
            None,
        ),
        0b101111 => ("SError interrupt", None),
        0b110000 => ("Breakpoint exception from a lower Exception level", None),
        0b110001 => (
            "Breakpoint exception taken without a change in Exception level",
            None,
        ),
        0b110010 => ("Software Step exception from a lower Exception level", None),
        0b110011 => (
            "Software Step exception taken without a change in Exception level",
            None,
        ),
        0b110100 => ("Watchpoint exception from a lower Exception level", None),
        0b110101 => (
            "Watchpoint exception taken without a change in Exception level",
            None,
        ),
        0b111000 => ("BKPT instruction execution in AArch32 state", None),
        0b111100 => ("BRK instruction execution in AArch64 state", None),
        _ => return Err(DecodeError::InvalidEc { ec: ec.value }),
    };
    let iss = FieldInfo {
        decoded: iss_decoded,
        ..iss
    };
    let ec = ec.with_decoded(Decoded {
        description: Some(class.to_string()),
        fields: vec![],
    });
    Ok(Decoded {
        description: Some(class.to_string()),
        fields: vec![res0, iss2, ec, il, iss],
    })
}

fn describe_il(il: bool) -> &'static str {
    if il {
        "32-bit instruction trapped"
    } else {
        "16-bit instruction trapped"
    }
}

/// Parses a decimal or hexadecimal number from a string.
///
/// If the string starts with `"0x"` then it will be parsed as hexadecimal, otherwise it will be
/// assumed to be decimal.
pub fn parse_number(s: &str) -> Result<u64, ParseIntError> {
    if let Some(hex) = s.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
    } else {
        s.parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_decimal() {
        assert_eq!(parse_number("12345"), Ok(12345));
    }

    #[test]
    fn parse_hex() {
        assert_eq!(parse_number("0x123abc"), Ok(0x123abc));
    }

    #[test]
    fn parse_invalid() {
        assert!(parse_number("123abc").is_err());
    }

    #[test]
    fn unknown() {
        let decoded = decode(0).unwrap();
        assert_eq!(
            decoded,
            Decoded {
                description: Some("Unknown reason".to_string()),
                fields: vec![
                    FieldInfo {
                        name: "RES0",
                        start: 37,
                        width: 27,
                        value: 0,
                        decoded: None,
                    },
                    FieldInfo {
                        name: "ISS2",
                        start: 32,
                        width: 5,
                        value: 0,
                        decoded: None,
                    },
                    FieldInfo {
                        name: "EC",
                        start: 26,
                        width: 6,
                        value: 0,
                        decoded: Some(Decoded {
                            description: Some("Unknown reason".to_string()),
                            fields: vec![],
                        })
                    },
                    FieldInfo {
                        name: "IL",
                        start: 25,
                        width: 1,
                        value: 0,
                        decoded: Some(Decoded {
                            description: Some("16-bit instruction trapped".to_string()),
                            fields: vec![],
                        })
                    },
                    FieldInfo {
                        name: "ISS",
                        start: 0,
                        width: 25,
                        value: 0,
                        decoded: Some(Decoded {
                            description: Some("ISS is RES0".to_string()),
                            fields: vec![],
                        })
                    },
                ]
            }
        );
    }

    #[test]
    fn data_abort() {
        assert_eq!(
            decode(0x96000050).unwrap(),
            Decoded {
                description: Some(
                    "Data Abort taken without a change in Exception level".to_string()
                ),
                fields: vec![
                    FieldInfo {
                        name: "RES0",
                        start: 37,
                        width: 27,
                        value: 0,
                        decoded: None,
                    },
                    FieldInfo {
                        name: "ISS2",
                        start: 32,
                        width: 5,
                        value: 0,
                        decoded: None,
                    },
                    FieldInfo {
                        name: "EC",
                        start: 26,
                        width: 6,
                        value: 37,
                        decoded: Some(Decoded {
                            description: Some(
                                "Data Abort taken without a change in Exception level".to_string()
                            ),
                            fields: vec![],
                        })
                    },
                    FieldInfo {
                        name: "IL",
                        start: 25,
                        width: 1,
                        value: 1,
                        decoded: Some(Decoded {
                            description: Some("32-bit instruction trapped".to_string()),
                            fields: vec![],
                        })
                    },
                    FieldInfo {
                        name: "ISS",
                        start: 0,
                        width: 25,
                        value: 80,
                        decoded: Some(Decoded {
                            description: None,
                            fields: vec![
                                FieldInfo {
                                    name: "ISV",
                                    start: 24,
                                    width: 1,
                                    value: 0,
                                    decoded: Some(Decoded {
                                        description: Some(
                                            "No valid instruction syndrome".to_string()
                                        ),
                                        fields: vec![],
                                    })
                                },
                                FieldInfo {
                                    name: "RES0",
                                    start: 14,
                                    width: 10,
                                    value: 0,
                                    decoded: None,
                                },
                                FieldInfo {
                                    name: "VNCR",
                                    start: 13,
                                    width: 1,
                                    value: 0,
                                    decoded: None,
                                },
                                FieldInfo {
                                    name: "SET",
                                    start: 11,
                                    width: 2,
                                    value: 0,
                                    decoded: Some(Decoded {
                                        description: Some("Recoverable state (UER)".to_string()),
                                        fields: vec![],
                                    })
                                },
                                FieldInfo {
                                    name: "FnV",
                                    start: 10,
                                    width: 1,
                                    value: 0,
                                    decoded: Some(Decoded {
                                        description: Some("FAR is valid".to_string()),
                                        fields: vec![],
                                    })
                                },
                                FieldInfo {
                                    name: "EA",
                                    start: 9,
                                    width: 1,
                                    value: 0,
                                    decoded: None,
                                },
                                FieldInfo {
                                    name: "CM",
                                    start: 8,
                                    width: 1,
                                    value: 0,
                                    decoded: None,
                                },
                                FieldInfo {
                                    name: "S1PTW",
                                    start: 7,
                                    width: 1,
                                    value: 0,
                                    decoded: None,
                                },
                                FieldInfo {
                                    name: "WnR",
                                    start: 6,
                                    width: 1,
                                    value: 1,
                                    decoded: Some(Decoded {
                                        description: Some(
                                            "Abort caused by writing to memory".to_string()
                                        ),
                                        fields: vec![],
                                    })
                                },
                                FieldInfo {
                                    name: "DFSC",
                                    start: 0,
                                    width: 6,
                                    value: 16,
                                    decoded: Some(Decoded {
                                        description: Some(
                                            "Synchronous External abort, not on translation table \
                                             walk or hardware update of translation table."
                                                .to_string()
                                        ),
                                        fields: vec![],
                                    })
                                }
                            ]
                        })
                    },
                ],
            }
        );
    }

    #[test]
    fn data_abort_isv() {
        assert_eq!(
            decode(0x97523050).unwrap(),
            Decoded {
                description: Some(
                    "Data Abort taken without a change in Exception level".to_string()
                ),
                fields: vec![
                    FieldInfo {
                        name: "RES0",
                        start: 37,
                        width: 27,
                        value: 0,
                        decoded: None,
                    },
                    FieldInfo {
                        name: "ISS2",
                        start: 32,
                        width: 5,
                        value: 0,
                        decoded: None,
                    },
                    FieldInfo {
                        name: "EC",
                        start: 26,
                        width: 6,
                        value: 37,
                        decoded: Some(Decoded {
                            description: Some(
                                "Data Abort taken without a change in Exception level".to_string()
                            ),
                            fields: vec![],
                        })
                    },
                    FieldInfo {
                        name: "IL",
                        start: 25,
                        width: 1,
                        value: 1,
                        decoded: Some(Decoded {
                            description: Some("32-bit instruction trapped".to_string()),
                            fields: vec![],
                        })
                    },
                    FieldInfo {
                        name: "ISS",
                        start: 0,
                        width: 25,
                        value: 22163536,
                        decoded: Some(Decoded {
                            description: None,
                            fields: vec![
                                FieldInfo {
                                    name: "ISV",
                                    start: 24,
                                    width: 1,
                                    value: 1,
                                    decoded: Some(Decoded {
                                        description: Some("Valid instruction syndrome".to_string()),
                                        fields: vec![],
                                    })
                                },
                                FieldInfo {
                                    name: "SAS",
                                    start: 22,
                                    width: 2,
                                    value: 1,
                                    decoded: Some(Decoded {
                                        description: Some("halfword".to_string()),
                                        fields: vec![],
                                    })
                                },
                                FieldInfo {
                                    name: "SSE",
                                    start: 21,
                                    width: 1,
                                    value: 0,
                                    decoded: None,
                                },
                                FieldInfo {
                                    name: "SRT",
                                    start: 16,
                                    width: 5,
                                    value: 18,
                                    decoded: None,
                                },
                                FieldInfo {
                                    name: "SF",
                                    start: 15,
                                    width: 1,
                                    value: 0,
                                    decoded: Some(Decoded {
                                        description: Some("32-bit wide register".to_string()),
                                        fields: vec![],
                                    })
                                },
                                FieldInfo {
                                    name: "AR",
                                    start: 14,
                                    width: 1,
                                    value: 0,
                                    decoded: Some(Decoded {
                                        description: Some(
                                            "No acquire/release semantics".to_string()
                                        ),
                                        fields: vec![],
                                    })
                                },
                                FieldInfo {
                                    name: "VNCR",
                                    start: 13,
                                    width: 1,
                                    value: 1,
                                    decoded: None,
                                },
                                FieldInfo {
                                    name: "SET",
                                    start: 11,
                                    width: 2,
                                    value: 2,
                                    decoded: Some(Decoded {
                                        description: Some("Uncontainable (UC)".to_string()),
                                        fields: vec![],
                                    })
                                },
                                FieldInfo {
                                    name: "FnV",
                                    start: 10,
                                    width: 1,
                                    value: 0,
                                    decoded: Some(Decoded {
                                        description: Some("FAR is valid".to_string()),
                                        fields: vec![],
                                    })
                                },
                                FieldInfo {
                                    name: "EA",
                                    start: 9,
                                    width: 1,
                                    value: 0,
                                    decoded: None,
                                },
                                FieldInfo {
                                    name: "CM",
                                    start: 8,
                                    width: 1,
                                    value: 0,
                                    decoded: None,
                                },
                                FieldInfo {
                                    name: "S1PTW",
                                    start: 7,
                                    width: 1,
                                    value: 0,
                                    decoded: None,
                                },
                                FieldInfo {
                                    name: "WnR",
                                    start: 6,
                                    width: 1,
                                    value: 1,
                                    decoded: Some(Decoded {
                                        description: Some(
                                            "Abort caused by writing to memory".to_string()
                                        ),
                                        fields: vec![],
                                    })
                                },
                                FieldInfo {
                                    name: "DFSC",
                                    start: 0,
                                    width: 6,
                                    value: 16,
                                    decoded: Some(Decoded {
                                        description: Some(
                                            "Synchronous External abort, not on translation table \
                                             walk or hardware update of translation table."
                                                .to_string()
                                        ),
                                        fields: vec![],
                                    })
                                }
                            ]
                        })
                    }
                ],
            }
        );
    }

    #[test]
    fn instruction_abort() {
        assert_eq!(
            decode(0x82001e10).unwrap(),
            Decoded {
                description: Some("Instruction Abort from a lower Exception level".to_string()),
                fields: vec![
                    FieldInfo {
                        name: "RES0",
                        start: 37,
                        width: 27,
                        value: 0,
                        decoded: None,
                    },
                    FieldInfo {
                        name: "ISS2",
                        start: 32,
                        width: 5,
                        value: 0,
                        decoded: None,
                    },
                    FieldInfo {
                        name: "EC",
                        start: 26,
                        width: 6,
                        value: 32,
                        decoded: Some(Decoded {
                            description: Some(
                                "Instruction Abort from a lower Exception level".to_string()
                            ),
                            fields: vec![],
                        })
                    },
                    FieldInfo {
                        name: "IL",
                        start: 25,
                        width: 1,
                        value: 1,
                        decoded: Some(Decoded {
                            description: Some("32-bit instruction trapped".to_string()),
                            fields: vec![],
                        })
                    },
                    FieldInfo {
                        name: "ISS",
                        start: 0,
                        width: 25,
                        value: 7696,
                        decoded: Some(Decoded {
                            description: None,
                            fields: vec![
                                FieldInfo {
                                    name: "RES0",
                                    start: 13,
                                    width: 12,
                                    value: 0,
                                    decoded: None,
                                },
                                FieldInfo {
                                    name: "SET",
                                    start: 11,
                                    width: 2,
                                    value: 3,
                                    decoded: Some(Decoded {
                                        description: Some("Restartable state (UEO)".to_string()),
                                        fields: vec![],
                                    })
                                },
                                FieldInfo {
                                    name: "FnV",
                                    start: 10,
                                    width: 1,
                                    value: 1,
                                    decoded: Some(Decoded {
                                        description: Some(
                                            "FAR is not valid, it holds an unknown value"
                                                .to_string()
                                        ),
                                        fields: vec![],
                                    })
                                },
                                FieldInfo {
                                    name: "EA",
                                    start: 9,
                                    width: 1,
                                    value: 1,
                                    decoded: None,
                                },
                                FieldInfo {
                                    name: "RES0",
                                    start: 8,
                                    width: 1,
                                    value: 0,
                                    decoded: None,
                                },
                                FieldInfo {
                                    name: "S1PTW",
                                    start: 7,
                                    width: 1,
                                    value: 0,
                                    decoded: None,
                                },
                                FieldInfo {
                                    name: "RES0",
                                    start: 6,
                                    width: 1,
                                    value: 0,
                                    decoded: None,
                                },
                                FieldInfo {
                                    name: "IFSC",
                                    start: 0,
                                    width: 6,
                                    value: 16,
                                    decoded: Some(Decoded {
                                        description: Some(
                                            "Synchronous External abort, not on translation table \
                                             walk or hardware update of translation table."
                                                .to_string()
                                        ),
                                        fields: vec![],
                                    })
                                }
                            ]
                        })
                    }
                ]
            }
        );
    }
}

use std::collections::HashMap;

use serde::Deserialize;
use toml::Value;

use crate::bluestation::{CfgSoapySdr, SoapySdrDto};

/// The PHY layer backend type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum PhyBackend {
    Undefined,
    None,
    SoapySdr,
}

/// PHY layer I/O configuration
#[derive(Debug, Clone)]
pub struct CfgPhyIo {
    /// Backend type: Soapysdr, File, or None
    pub backend: PhyBackend,

    pub dl_tx_file: Option<String>,
    pub ul_rx_file: Option<String>,
    pub ul_input_file: Option<String>,
    pub dl_input_file: Option<String>,

    /// For Soapysdr backend: SoapySDR configuration
    pub soapysdr: Option<CfgSoapySdr>,
}

#[derive(Deserialize)]
pub struct PhyIoDto {
    pub backend: PhyBackend,

    pub dl_tx_file: Option<String>,
    pub ul_rx_file: Option<String>,
    pub ul_input_file: Option<String>,
    pub dl_input_file: Option<String>,

    pub soapysdr: Option<SoapySdrDto>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

pub fn phy_dto_to_cfg(src: PhyIoDto) -> Result<CfgPhyIo, String> {
    let soapysdr = src
        .soapysdr
        .map(|soapy_dto| {
            let rx_gains = soapy_dto
                .extra
                .iter()
                .filter_map(|(key, value)| key.strip_prefix("rx_gain_").map(|gain_name| (key, gain_name, value)))
                .map(|(key, gain_name, value)| {
                    let gain_value = match value {
                        Value::Integer(v) => *v as f64,
                        Value::Float(v) => *v,
                        _ => return Err(format!("phy_io.soapysdr.{key} must be a number")),
                    };
                    Ok((gain_name.to_string().to_lowercase(), gain_value))
                })
                .collect::<Result<HashMap<_, _>, String>>()?;

            let tx_gains = soapy_dto
                .extra
                .iter()
                .filter_map(|(key, value)| key.strip_prefix("tx_gain_").map(|gain_name| (key, gain_name, value)))
                .map(|(key, gain_name, value)| {
                    let gain_value = match value {
                        Value::Integer(v) => *v as f64,
                        Value::Float(v) => *v,
                        _ => return Err(format!("phy_io.soapysdr.{key} must be a number")),
                    };
                    Ok((gain_name.to_string().to_lowercase(), gain_value))
                })
                .collect::<Result<HashMap<_, _>, String>>()?;

            Ok::<CfgSoapySdr, String>(CfgSoapySdr {
                ul_freq: soapy_dto.rx_freq,
                dl_freq: soapy_dto.tx_freq,
                ppm_err: soapy_dto.ppm_err.unwrap_or(0.0),
                device: soapy_dto.device,
                fs: soapy_dto.sample_rate,
                rx_ch: soapy_dto.rx_channel,
                tx_ch: soapy_dto.tx_channel,
                rx_ant: soapy_dto.rx_antenna,
                tx_ant: soapy_dto.tx_antenna,
                rx_gains,
                tx_gains,
                tx_calibration_enabled: soapy_dto.tx_calibration_enabled.unwrap_or(false),
                tx_calibration_file: soapy_dto
                    .tx_calibration_file
                    .unwrap_or_else(|| crate::bluestation::TX_CALIBRATION_DEFAULT_FILE.to_string()),
                tx_calibration_apply_dc: soapy_dto.tx_calibration_apply_dc.unwrap_or(true),
                tx_calibration_apply_iq: soapy_dto.tx_calibration_apply_iq.unwrap_or(false),
            })
        })
        .transpose()?;

    Ok(CfgPhyIo {
        backend: src.backend,
        dl_tx_file: src.dl_tx_file,
        ul_rx_file: src.ul_rx_file,
        ul_input_file: src.ul_input_file,
        dl_input_file: src.dl_input_file,
        soapysdr,
    })
}

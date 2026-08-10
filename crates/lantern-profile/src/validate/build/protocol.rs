use super::super::{helpers::*, *};

pub(super) fn validate_protocol(
    document: &mut ProfileDocumentV1,
) -> Result<ValidatedProtocol, ProfileError> {
    let protocol = &mut document.protocol;
    let default_baud_rate = BaudRate::new(protocol.default_baud_rate)
        .map_err(|error| ProfileError::validation("protocol.default_baud_rate", error))?;
    let default_parity = parity(protocol.default_parity);
    let default_data_bits = data_bits(protocol.default_data_bits, "protocol.default_data_bits")?;
    let default_stop_bits = stop_bits(protocol.default_stop_bits, "protocol.default_stop_bits")?;
    let slave_id = SlaveId::new(protocol.default_slave_id)
        .map_err(|error| ProfileError::validation("protocol.default_slave_id", error))?;
    if protocol.response_timeout_ms == 0 {
        return Err(ProfileError::validation(
            "protocol.response_timeout_ms",
            "timeout must be non-zero",
        ));
    }

    protocol.allowed_baud_rates.push(protocol.default_baud_rate);
    protocol.allowed_baud_rates.sort_unstable();
    protocol.allowed_baud_rates.dedup();
    let allowed_baud_rates = protocol
        .allowed_baud_rates
        .iter()
        .enumerate()
        .map(|(index, value)| {
            BaudRate::new(*value).map_err(|error| {
                ProfileError::validation(format!("protocol.allowed_baud_rates[{index}]"), error)
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    protocol.allowed_parities.push(protocol.default_parity);
    protocol
        .allowed_parities
        .sort_by_key(|value| parity_rank(*value));
    protocol
        .allowed_parities
        .dedup_by_key(|value| parity_rank(*value));
    let allowed_parities = protocol
        .allowed_parities
        .iter()
        .copied()
        .map(parity)
        .collect::<Vec<_>>();

    protocol.allowed_data_bits.push(protocol.default_data_bits);
    protocol.allowed_data_bits.sort_unstable();
    protocol.allowed_data_bits.dedup();
    let allowed_data_bits = protocol
        .allowed_data_bits
        .iter()
        .enumerate()
        .map(|(index, value)| data_bits(*value, format!("protocol.allowed_data_bits[{index}]")))
        .collect::<Result<Vec<_>, _>>()?;

    protocol.allowed_stop_bits.push(protocol.default_stop_bits);
    protocol.allowed_stop_bits.sort_unstable();
    protocol.allowed_stop_bits.dedup();
    let allowed_stop_bits = protocol
        .allowed_stop_bits
        .iter()
        .enumerate()
        .map(|(index, value)| stop_bits(*value, format!("protocol.allowed_stop_bits[{index}]")))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ValidatedProtocol {
        default_link: LinkSettings {
            baud_rate: default_baud_rate,
            parity: default_parity,
            data_bits: default_data_bits,
            stop_bits: default_stop_bits,
            response_timeout: Duration::from_millis(protocol.response_timeout_ms),
            slave_id,
            rs485_mode: match protocol.rs485_mode {
                Rs485ModeDocument::AdapterManaged => Rs485Mode::AdapterManaged,
                Rs485ModeDocument::LinuxIoctl => Rs485Mode::LinuxIoctl,
            },
        },
        allowed_baud_rates: allowed_baud_rates.into_boxed_slice(),
        allowed_parities: allowed_parities.into_boxed_slice(),
        allowed_data_bits: allowed_data_bits.into_boxed_slice(),
        allowed_stop_bits: allowed_stop_bits.into_boxed_slice(),
        minimum_inter_frame_delay: Duration::from_micros(protocol.minimum_inter_frame_delay_us),
    })
}

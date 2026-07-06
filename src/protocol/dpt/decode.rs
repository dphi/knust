use super::dpt_type::DptType;
use super::payload::DptPayload;
use super::unit::Unit;
use super::view::DptView;
use crate::error::Result;
use crate::log_protocol;
use crate::logging::LogLevel;
use crate::protocol::address::{GroupAddress, IndividualAddress};
use crate::protocol::telegram::{Telegram, TelegramType};

/// A fully decoded telegram with type-safe access to all fields.
pub struct DecodedTelegram<'a> {
    pub source: IndividualAddress,
    pub destination: GroupAddress,
    pub telegram_type: TelegramType,
    pub dpt: DptType,
    pub view: DptView<'a>,
    pub gateway_id: Option<u16>,
    pub group_name: Option<&'a str>,
}

impl<'a> DecodedTelegram<'a> {
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        self.view.raw()
    }
    #[must_use]
    pub fn numeric(&self) -> Option<f64> {
        self.view.as_f64()
    }
    #[must_use]
    pub fn unit(&self) -> Option<Unit> {
        self.dpt.unit()
    }
    #[must_use]
    pub fn formatted(&self) -> String {
        let payload = DptPayload::from(self.view);
        payload.formatted(self.dpt.unit())
    }
    #[must_use]
    pub fn to_owned_payload(&self) -> DptPayload {
        DptPayload::from(self.view)
    }
    #[must_use]
    pub fn is_write(&self) -> bool {
        self.telegram_type == TelegramType::GroupValueWrite
    }
    #[must_use]
    pub fn is_response(&self) -> bool {
        self.telegram_type == TelegramType::GroupValueResponse
    }
    #[must_use]
    pub fn is_read(&self) -> bool {
        self.telegram_type == TelegramType::GroupValueRead
    }
}

/// Decode a telegram given its DPT type.
///
/// # Errors
///
/// Returns [`ProtocolError::DptError`](crate::error::ProtocolError::DptError)
/// if `telegram.destination` is not a group address, or if the payload
/// doesn't match `dpt`'s expected encoding.
pub fn decode_telegram<'a>(
    telegram: &'a Telegram,
    dpt: DptType,
    group_name: Option<&'a str>,
) -> Result<DecodedTelegram<'a>> {
    let crate::protocol::address::Address::Group(destination) = telegram.destination else {
        return Err(crate::error::ProtocolError::DptError {
            dpt_type: dpt.number_str(),
            details: "telegram destination is not a group address".to_string(),
        }
        .into());
    };

    let view = match dpt.decode_ref(&telegram.payload) {
        Ok(v) => v,
        Err(e) => {
            log_protocol!(
                LogLevel::Warn,
                "DPT {} decode failed for telegram {} \u{2192} {}: {}",
                dpt.number_str(),
                telegram.source,
                destination,
                e,
            );
            return Err(e);
        }
    };

    log_protocol!(
        LogLevel::Debug,
        "\u{2190} {} {} \u{2192} {} '{}' [DPT {}] = {}",
        telegram.telegram_type,
        telegram.source,
        destination,
        group_name.unwrap_or("?"),
        dpt.number_str(),
        view.formatted(dpt),
    );

    Ok(DecodedTelegram {
        source: telegram.source,
        destination,
        telegram_type: telegram.telegram_type,
        dpt,
        view,
        gateway_id: telegram.gateway_id,
        group_name,
    })
}

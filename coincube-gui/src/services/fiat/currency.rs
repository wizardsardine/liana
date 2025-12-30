macro_rules! currency_enum {
    ($name:ident { $($variant:ident),* $(,)? }) => {
        #[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default)]
        pub enum $name {
            #[default]
            $($variant,)*
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    $(Self::$variant => write!(f, stringify!($variant)),)*
                }
            }
        }

        impl std::str::FromStr for $name {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s.to_uppercase().as_str() {
                    $(stringify!($variant) => Ok(Self::$variant),)*
                    _ => Err("Invalid currency".to_string()),
                }
            }
        }
    };
}

currency_enum!(Currency {
    USD, // macro sets first variant as the default
    AED,
    AMD,
    ARS,
    AUD,
    BAM,
    BDT,
    BHD,
    BMD,
    BRL,
    CAD,
    CHF,
    CLP,
    CNY,
    COP,
    CRC,
    CZK,
    DKK,
    DOP,
    EUR,
    GBP,
    GEL,
    GTQ,
    HKD,
    HNL,
    HUF,
    IDR,
    ILS,
    INR,
    JPY,
    KES,
    KRW,
    KWD,
    LKR,
    LBP,
    MMK,
    MXN,
    MYR,
    NGN,
    NOK,
    NZD,
    PEN,
    PHP,
    PKR,
    PLN,
    RON,
    RUB,
    SAR,
    SEK,
    SGD,
    SVC,
    THB,
    TRY,
    TWD,
    UAH,
    VEF,
    VND,
    ZAR,
    ZMW,
});

impl Currency {
    /// Returns the number of decimals required for the minor unit.
    pub fn decimals(&self) -> usize {
        match self {
            Currency::CLP | Currency::JPY | Currency::KRW | Currency::VND => 0,
            Currency::BHD | Currency::KWD => 3,
            _ => 2,
        }
    }
    pub fn to_static_str(&self) -> &'static str {
        match *self {
            Currency::USD => "USD",
            Currency::AED => "AED",
            Currency::AMD => "AMD",
            Currency::ARS => "ARS",
            Currency::AUD => "AUD",
            Currency::BAM => "BAM",
            Currency::BDT => "BDT",
            Currency::BHD => "BHD",
            Currency::BMD => "BMD",
            Currency::BRL => "BRL",
            Currency::CAD => "CAD",
            Currency::CHF => "CHF",
            Currency::CLP => "CLP",
            Currency::CNY => "CNY",
            Currency::COP => "COP",
            Currency::CRC => "CRC",
            Currency::CZK => "CZK",
            Currency::DKK => "DKK",
            Currency::DOP => "DOP",
            Currency::EUR => "EUR",
            Currency::GBP => "GBP",
            Currency::GEL => "GEL",
            Currency::GTQ => "GTQ",
            Currency::HKD => "HKD",
            Currency::HNL => "HNL",
            Currency::HUF => "HUF",
            Currency::IDR => "IDR",
            Currency::ILS => "ILS",
            Currency::INR => "INR",
            Currency::JPY => "JPY",
            Currency::KES => "KES",
            Currency::KRW => "KRW",
            Currency::KWD => "KWD",
            Currency::LKR => "LKR",
            Currency::LBP => "LBP",
            Currency::MMK => "MMK",
            Currency::MXN => "MXN",
            Currency::MYR => "MYR",
            Currency::NGN => "NGN",
            Currency::NOK => "NOK",
            Currency::NZD => "NZD",
            Currency::PEN => "PEN",
            Currency::PHP => "PHP",
            Currency::PKR => "PKR",
            Currency::PLN => "PLN",
            Currency::RON => "RON",
            Currency::RUB => "RUB",
            Currency::SAR => "SAR",
            Currency::SEK => "SEK",
            Currency::SGD => "SGD",
            Currency::SVC => "SVC",
            Currency::THB => "THB",
            Currency::TRY => "TRY",
            Currency::TWD => "TWD",
            Currency::UAH => "UAH",
            Currency::VEF => "VEF",
            Currency::VND => "VND",
            Currency::ZAR => "ZAR",
            Currency::ZMW => "ZMW",
        }
    }
}

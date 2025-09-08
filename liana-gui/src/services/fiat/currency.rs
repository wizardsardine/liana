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

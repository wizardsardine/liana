//! Optional personality for passive desktop surfaces. Technical copy keeps its
//! own catalog, and the language of a voice line follows the device locale.

use std::cell::Cell;

use crate::{locale, theme::WorldviewMode};

thread_local! {
    static MODE: Cell<WorldviewMode> = const { Cell::new(WorldviewMode::Samurai) };
}

/// Scope a view build to its window's accepted worldview.
pub fn render_in<T>(mode: WorldviewMode, render: impl FnOnce() -> T) -> T {
    struct Restore(WorldviewMode);
    impl Drop for Restore {
        fn drop(&mut self) {
            MODE.set(self.0);
        }
    }

    let _restore = Restore(MODE.replace(mode));
    render()
}

pub fn mode() -> WorldviewMode {
    MODE.get()
}

/// Only presentation contexts belong here. The caller owns the facts that
/// make a context eligible, such as a known, visible zero balance.
#[derive(Debug, Clone, Copy)]
pub enum VoiceLine {
    Profile,
    Consensus,
    ModeChanged,
    Welcome,
    Overview,
    CreateCube,
    CreateVault,
    EmptySha256Vault,
    EmptyBlake2bVault,
    EmptyHistory,
    Syncing,
    PaymentSent,
    PaymentReceived,
}

impl VoiceLine {
    pub fn get(self) -> Option<&'static str> {
        self.for_mode(mode())
    }

    pub fn for_mode(self, mode: WorldviewMode) -> Option<&'static str> {
        use VoiceLine::*;
        use WorldviewMode::*;

        let (english, spanish) = match (self, mode) {
            (Profile, Samurai) => (
                "Quiet discipline. Elegant simplicity. Tend to what is yours to control.",
                "Disciplina serena. Sencillez elegante. Ocúpate de lo que está en tu control.",
            ),
            (Profile, Suitcoiner) => (
                "Institutions are here. Number go up. Everything is fine.",
                "Las instituciones ya llegaron. El precio sube. Todo está bien.",
            ),
            (Profile, Luke) => (
                "The rules are sacred. The ledger must remain pure.",
                "Las reglas son sagradas. El registro debe mantenerse puro.",
            ),
            (Consensus, Suitcoiner) => (
                "Consensus by market capitalization.",
                "Consenso por capitalización de mercado.",
            ),
            (Consensus, Luke) => (
                "Consensus is not a popularity contest.",
                "El consenso no es un concurso de popularidad.",
            ),
            (ModeChanged, Samurai) => (
                "Ideological neutrality restored.",
                "Se restableció la neutralidad ideológica.",
            ),
            (ModeChanged, Suitcoiner) => (
                "Institutional wisdom activated.",
                "Sabiduría institucional activada.",
            ),
            (ModeChanged, Luke) => (
                "You have rejected the establishment.",
                "Has rechazado al poder establecido.",
            ),
            (Welcome, Samurai) => (
                "Welcome. Begin when you are ready.",
                "Te damos la bienvenida. Empieza cuando quieras.",
            ),
            (Welcome, Suitcoiner) => (
                "Welcome, institutional-grade HODLer.",
                "Te damos la bienvenida, HODLer de categoría institucional.",
            ),
            (Welcome, Luke) => (
                "Welcome, sovereign individual.",
                "Te damos la bienvenida, individuo soberano.",
            ),
            (Overview, Samurai) => ("Tend to what is yours.", "Cuida lo que es tuyo."),
            (Overview, Suitcoiner) => ("Your Bitcoin empire.", "Tu imperio de Bitcoin."),
            (Overview, Luke) => ("Your sovereign treasury.", "Tu tesorería soberana."),
            (CreateCube, Samurai) => (
                "Start with a clear foundation.",
                "Empieza con una base sólida.",
            ),
            (CreateCube, Suitcoiner) => {
                ("Establish a new position.", "Establece una nueva posición.")
            }
            (CreateCube, Luke) => ("Establish your sovereignty.", "Afirma tu soberanía."),
            (CreateVault, Samurai) => (
                "Prepare in calm for what may come.",
                "Prepárate con calma para lo que pueda venir.",
            ),
            (CreateVault, Suitcoiner) => (
                "The institutions have custodians. You have better options.",
                "Las instituciones tienen custodios. Tú tienes mejores opciones.",
            ),
            (CreateVault, Luke) => (
                "Guard your keys. Enforce your rules.",
                "Protege tus claves. Haz cumplir tus reglas.",
            ),
            // Samurai takes no side between the two chains.
            (EmptySha256Vault | EmptyBlake2bVault, Samurai) => (
                "Empty and ready. Fill it with patience.",
                "Vacío y listo. Llénalo con paciencia.",
            ),
            (EmptySha256Vault, Suitcoiner) => {
                ("Time to stack some sats.", "Es hora de acumular sats.")
            }
            (EmptySha256Vault, Luke) => (
                "No Spamcoin detected. Excellent.",
                "No se detectó Spamcoin. Excelente.",
            ),
            (EmptyBlake2bVault, Suitcoiner) => (
                "Your portfolio remains institutionally compliant.",
                "Tu cartera sigue cumpliendo las normas institucionales.",
            ),
            (EmptyBlake2bVault, Luke) => (
                "A clean ledger begins with a single transaction.",
                "Un registro limpio comienza con una sola transacción.",
            ),
            (EmptyHistory, Samurai) => (
                "Every path begins with a first step.",
                "Todo camino empieza con un primer paso.",
            ),
            (EmptyHistory, Suitcoiner) => (
                "No economic activity recorded. Yet.",
                "Sin actividad económica registrada. Por ahora.",
            ),
            (EmptyHistory, Luke) => (
                "The ledger awaits its first honest payment.",
                "El registro espera su primer pago honesto.",
            ),
            (Syncing, Samurai) => (
                "Patience is part of the way.",
                "La paciencia también es parte del camino.",
            ),
            (Syncing, Suitcoiner) => ("Consulting the market…", "Consultando al mercado…"),
            (Syncing, Luke) => (
                "Consulting the ancient rules…",
                "Consultando las antiguas reglas…",
            ),
            // Broadcast is not confirmation, so the line claims only intent.
            (PaymentSent, Samurai) => ("Sent with intention.", "Enviado con intención."),
            (PaymentSent, Suitcoiner) => (
                "The free market thanks you.",
                "El libre mercado te lo agradece.",
            ),
            (PaymentSent, Luke) => ("No JPEGs were required.", "No hizo falta ningún JPEG."),
            (PaymentReceived, Samurai) => ("Received with gratitude.", "Recibido con gratitud."),
            (PaymentReceived, Suitcoiner) => {
                ("Capital inflow recorded.", "Entrada de capital registrada.")
            }
            (PaymentReceived, Luke) => (
                "An honest payment has arrived.",
                "Ha llegado un pago honesto.",
            ),
            // Consensus is the Suitcoiner/Luke rivalry; the neutral voice
            // stays out of it.
            (Consensus, Samurai) => return None,
        };

        Some(match locale::language() {
            locale::Language::English => english,
            locale::Language::Spanish => spanish,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worldview_voice_is_independent_of_language_and_defaults_to_samurai() {
        assert_eq!(VoiceLine::PaymentSent.get(), Some("Sent with intention."));
        render_in(WorldviewMode::Suitcoiner, || {
            assert_eq!(
                VoiceLine::Consensus.get(),
                Some("Consensus by market capitalization.")
            );
            locale::render_in(locale::Language::Spanish, || {
                assert_eq!(
                    VoiceLine::Consensus.get(),
                    Some("Consenso por capitalización de mercado.")
                );
                render_in(WorldviewMode::Luke, || {
                    assert_eq!(
                        VoiceLine::Consensus.get(),
                        Some("El consenso no es un concurso de popularidad.")
                    );
                });
            });
            assert_eq!(mode(), WorldviewMode::Suitcoiner);
        });
        assert_eq!(mode(), WorldviewMode::Samurai);
    }

    #[test]
    fn samurai_speaks_everywhere_but_the_consensus_rivalry() {
        use VoiceLine::*;

        let all = [
            Profile,
            Consensus,
            ModeChanged,
            Welcome,
            Overview,
            CreateCube,
            CreateVault,
            EmptySha256Vault,
            EmptyBlake2bVault,
            EmptyHistory,
            Syncing,
            PaymentSent,
            PaymentReceived,
        ];
        for language in [locale::Language::English, locale::Language::Spanish] {
            locale::render_in(language, || {
                for line in all {
                    let samurai = line.for_mode(WorldviewMode::Samurai);
                    assert_eq!(
                        samurai.is_none(),
                        matches!(line, Consensus),
                        "{line:?} in {language:?}"
                    );
                    for mode in [WorldviewMode::Suitcoiner, WorldviewMode::Luke] {
                        assert!(line.for_mode(mode).is_some(), "{line:?} {mode:?}");
                    }
                }
            });
        }
        assert_eq!(
            Overview.for_mode(WorldviewMode::Samurai),
            Some("Tend to what is yours.")
        );
        locale::render_in(locale::Language::Spanish, || {
            assert_eq!(
                Overview.for_mode(WorldviewMode::Samurai),
                Some("Cuida lo que es tuyo.")
            );
        });
    }
}

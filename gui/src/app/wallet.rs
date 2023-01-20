use liana::descriptors::MultipathDescriptor;

#[derive(Clone)]
pub struct Wallet {
    pub name: String,
    pub main_descriptor: MultipathDescriptor,
}

impl Wallet {
    pub fn new(main_descriptor: MultipathDescriptor) -> Self {
        Self {
            name: "Liana".to_string(),
            main_descriptor,
        }
    }
}

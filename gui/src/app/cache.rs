use crate::daemon::model::Coin;

pub struct Cache {
    pub blockheight: i32,
    pub coins: Vec<Coin>,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            blockheight: 0,
            coins: Vec::new(),
        }
    }
}

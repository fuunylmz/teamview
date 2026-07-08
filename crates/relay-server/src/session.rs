use teamview_protocol::control::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: u64,
    pub user_id: Option<UserId>,
}

impl Session {
    pub fn anonymous(id: u64) -> Self {
        Self { id, user_id: None }
    }

    pub fn authenticate(&mut self, user_id: UserId) {
        self.user_id = Some(user_id);
    }
}

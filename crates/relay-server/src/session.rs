use teamview_protocol::control::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: u64,
    pub user_id: Option<UserId>,
    pub access_granted: bool,
}

impl Session {
    pub fn anonymous(id: u64) -> Self {
        Self {
            id,
            user_id: None,
            access_granted: false,
        }
    }

    pub fn establish_identity(&mut self, user_id: UserId, access_granted: bool) {
        self.user_id = Some(user_id);
        self.access_granted = access_granted;
    }

    pub fn grant_access(&mut self) {
        self.access_granted = true;
    }

    pub fn has_access(&self) -> bool {
        self.user_id.is_some() && self.access_granted
    }
}

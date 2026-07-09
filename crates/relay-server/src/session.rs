use teamview_protocol::control::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: u64,
    pub user_id: Option<UserId>,
    pub display_name: String,
    pub access_granted: bool,
}

impl Session {
    pub fn anonymous(id: u64) -> Self {
        Self {
            id,
            user_id: None,
            display_name: format!("session-{id}"),
            access_granted: false,
        }
    }

    pub fn establish_identity(
        &mut self,
        user_id: UserId,
        access_granted: bool,
        display_name: impl Into<String>,
    ) {
        self.user_id = Some(user_id);
        self.display_name = display_name.into();
        self.access_granted = access_granted;
    }

    pub fn update_display_name(&mut self, display_name: impl Into<String>) {
        self.display_name = display_name.into();
    }

    pub fn grant_access(&mut self) {
        self.access_granted = true;
    }

    pub fn has_access(&self) -> bool {
        self.user_id.is_some() && self.access_granted
    }
}

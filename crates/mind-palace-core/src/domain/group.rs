use serde::{Deserialize, Serialize};

use super::value_objects::GroupId;

/// A first-class group of users (Spec 2 §2.2).
///
/// - `members` receive any grant issued to `Principal::Group(id)`.
/// - `managers` are the only principals allowed to mutate membership. This is
///   the privileged-operation boundary that prevents the Outline-class
///   escalation (Spec 2 §5): a regular member cannot add members or add
///   themselves; only a manager can.
///
/// Groups are generic. A multi-tenant "tenant" is just a group whose members
/// are its users (Spec 2 §6) — there is no company/DashLX-specific concept here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    /// Member emails (stable identity strings). Members inherit group grants.
    pub members: Vec<String>,
    /// Manager emails. Only managers may modify membership.
    pub managers: Vec<String>,
}

impl Group {
    /// Create a new group with `creator` as the sole initial manager and member
    /// (Spec 2 §5: "Group creation sets the creator as the sole initial manager").
    pub fn new(id: GroupId, name: impl Into<String>, creator: impl Into<String>) -> Self {
        let creator = creator.into();
        Self {
            id,
            name: name.into(),
            members: vec![creator.clone()],
            managers: vec![creator],
        }
    }

    /// True if `email` is a member (managers are always members too by convention,
    /// but membership is checked explicitly against the `members` list).
    pub fn is_member(&self, email: &str) -> bool {
        self.members.iter().any(|m| m == email)
    }

    /// True if `email` is a manager and thus allowed to mutate membership.
    pub fn is_manager(&self, email: &str) -> bool {
        self.managers.iter().any(|m| m == email)
    }

    /// Add a member (idempotent). Authorization is enforced by the service, not here.
    pub fn add_member(&mut self, email: impl Into<String>) {
        let email = email.into();
        if !self.is_member(&email) {
            self.members.push(email);
        }
    }

    /// Remove a member (idempotent).
    pub fn remove_member(&mut self, email: &str) {
        self.members.retain(|m| m != email);
    }
}

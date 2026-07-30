use super::{Error, Mutex};

pub(super) fn set_failure(slot: &Mutex<Option<Error>>, error: Error) {
    let mut slot = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if slot.is_none() {
        *slot = Some(error);
    }
}

pub(super) fn has_failure(slot: &Mutex<Option<Error>>) -> bool {
    slot.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_some()
}

pub(super) fn take_failure(slot: &Mutex<Option<Error>>) -> Option<Error> {
    slot.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

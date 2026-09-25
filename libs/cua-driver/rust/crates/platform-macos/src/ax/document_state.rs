use std::time::{Duration, Instant};

pub const DOCUMENT_STATE_BUDGET: Duration = Duration::from_millis(500);

pub struct Deadline {
    end: Instant,
}

impl Deadline {
    pub fn after(budget: Duration) -> Self {
        Self {
            end: Instant::now() + budget,
        }
    }

    pub fn remaining(&self) -> Option<Duration> {
        self.end
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
    }

    pub fn messaging_timeout_seconds(&self) -> Option<f32> {
        Some(self.remaining()?.as_secs_f32())
    }
}

pub trait DocumentAttributes {
    fn document_url(&self, deadline: &Deadline) -> Option<String>;
    fn edited_on_window(&self, deadline: &Deadline) -> Option<bool>;
    fn edited_on_close_button(&self, deadline: &Deadline) -> Option<bool>;
}

#[derive(Debug, Default, PartialEq)]
pub struct DocumentState {
    pub path: Option<String>,
    pub edited: Option<bool>,
}

pub fn collect_within(attributes: &impl DocumentAttributes, budget: Duration) -> DocumentState {
    let deadline = Deadline::after(budget);

    let path = deadline
        .remaining()
        .and_then(|_| attributes.document_url(&deadline))
        .as_deref()
        .and_then(crate::file_url::local_path_from_file_url)
        .map(|path| path.to_string_lossy().into_owned());

    let edited = deadline
        .remaining()
        .and_then(|_| attributes.edited_on_window(&deadline))
        .or_else(|| {
            deadline
                .remaining()
                .and_then(|_| attributes.edited_on_close_button(&deadline))
        });

    DocumentState { path, edited }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const STALL: Duration = Duration::from_millis(120);

    #[derive(Default)]
    struct FakeApp {
        url: Option<String>,
        window_edited: Option<bool>,
        close_button_edited: Option<bool>,
        stalls: bool,
        calls: RefCell<Vec<&'static str>>,
        timeouts: RefCell<Vec<f32>>,
    }

    impl FakeApp {
        fn record(&self, attribute: &'static str, deadline: &Deadline) {
            self.calls.borrow_mut().push(attribute);
            if let Some(seconds) = deadline.messaging_timeout_seconds() {
                self.timeouts.borrow_mut().push(seconds);
            }
            if self.stalls {
                std::thread::sleep(STALL);
            }
        }
    }

    impl DocumentAttributes for FakeApp {
        fn document_url(&self, deadline: &Deadline) -> Option<String> {
            self.record("AXDocument", deadline);
            self.url.clone()
        }

        fn edited_on_window(&self, deadline: &Deadline) -> Option<bool> {
            self.record("AXEdited", deadline);
            self.window_edited
        }

        fn edited_on_close_button(&self, deadline: &Deadline) -> Option<bool> {
            self.record("AXCloseButton/AXEdited", deadline);
            self.close_button_edited
        }
    }

    #[test]
    fn each_request_is_given_only_the_time_that_is_left() {
        let slow = FakeApp {
            stalls: true,
            close_button_edited: Some(true),
            ..FakeApp::default()
        };
        let budget = STALL * 3;

        collect_within(&slow, budget);

        let timeouts = slow.timeouts.borrow().clone();
        assert_eq!(timeouts.len(), 3, "every attribute should have been read");
        assert!(
            timeouts[0] <= budget.as_secs_f32(),
            "first request exceeded the budget: {timeouts:?}"
        );
        assert!(
            timeouts.windows(2).all(|pair| pair[1] < pair[0]),
            "each request must be handed less time than the previous: {timeouts:?}"
        );
        assert!(
            timeouts[2] < (budget - STALL * 2).as_secs_f32() + 0.01,
            "the last request kept time already spent: {timeouts:?}"
        );
    }

    #[test]
    fn an_app_that_never_answers_costs_the_budget_once_not_once_per_attribute() {
        let hung = FakeApp {
            stalls: true,
            ..FakeApp::default()
        };
        let budget = STALL / 2;

        let started = Instant::now();
        let state = collect_within(&hung, budget);
        let elapsed = started.elapsed();

        assert_eq!(state, DocumentState::default());
        assert_eq!(
            hung.calls.borrow().as_slice(),
            ["AXDocument"],
            "reads after the deadline must be skipped"
        );
        assert!(
            elapsed < STALL * 2,
            "probe cost more than the one request already in flight: {elapsed:?}"
        );
    }

    #[test]
    fn a_responsive_app_reports_the_decoded_path_and_the_window_flag() {
        let app = FakeApp {
            url: Some("file:///Users/x/My%20Notes.txt".into()),
            window_edited: Some(true),
            ..FakeApp::default()
        };

        let state = collect_within(&app, DOCUMENT_STATE_BUDGET);

        assert_eq!(
            state,
            DocumentState {
                path: Some("/Users/x/My Notes.txt".into()),
                edited: Some(true),
            }
        );
        assert_eq!(app.calls.borrow().as_slice(), ["AXDocument", "AXEdited"]);
    }

    #[test]
    fn the_close_button_answers_for_the_appkit_windows_that_do_not() {
        let appkit = FakeApp {
            close_button_edited: Some(false),
            ..FakeApp::default()
        };

        let state = collect_within(&appkit, DOCUMENT_STATE_BUDGET);

        assert_eq!(state.edited, Some(false));
        assert_eq!(
            appkit.calls.borrow().as_slice(),
            ["AXDocument", "AXEdited", "AXCloseButton/AXEdited"]
        );
    }
}

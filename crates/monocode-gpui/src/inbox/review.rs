//! The reviewer's pending comments on the pull request on screen: added from
//! the diff, kept in settings, and posted together as one review.
use super::*;
use model::review::{self, PendingComment, Side};
use monocode_core::mobile::inbox::ReviewLine;

impl Inbox {
    /// The pending comments for the pull request on screen.
    pub(super) fn review_comments(&self) -> &[PendingComment] {
        self.item()
            .and_then(|item| self.review.get(&item.key()))
            .map_or(&[], Vec::as_slice)
    }

    /// Reads the saved comments of the pull request on screen.
    pub(super) fn load_review(&mut self, cx: &mut Context<Self>) {
        self.review_open = false;
        self.review_error = None;
        self.review_posted = None;
        let Some(key) = self.item().map(|item| item.key()) else {
            return;
        };
        if !self.review.contains_key(&key) {
            let saved = locked(&self.store)
                .ok()
                .and_then(|store| store.setting(&review::setting_key(&key)).ok().flatten())
                .map(|json| review::parse(&json))
                .unwrap_or_default();
            self.review.insert(key, saved);
        }
        self.sync_review_marks(cx);
    }

    fn sync_review_marks(&mut self, cx: &mut Context<Self>) {
        let marks = self
            .review_comments()
            .iter()
            .map(|comment| (comment.path.clone(), comment.line, comment.side))
            .collect();
        self.diff_view
            .update(cx, |view, cx| view.set_marks(marks, cx));
    }

    fn save_review(&self, key: &str) {
        let json = review::serialize(self.review.get(key).map_or(&[], Vec::as_slice));
        if let Ok(store) = locked(&self.store) {
            let _ = store.save_setting(&review::setting_key(key), &json);
        }
    }

    pub(super) fn add_review_comment(
        &mut self,
        path: String,
        line: u32,
        side: Side,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.item().map(|item| item.key()) else {
            return;
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        if !self.review.contains_key(&key) {
            self.load_review(cx);
        }
        let comments = self.review.entry(key.clone()).or_default();
        comments.push(PendingComment {
            id: review::next_id(comments),
            path,
            line,
            side,
            text,
        });
        self.review_posted = None;
        self.review_error = None;
        self.save_review(&key);
        self.sync_review_marks(cx);
        cx.notify();
    }

    pub(super) fn remove_review_comment(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(key) = self.item().map(|item| item.key()) else {
            return;
        };
        if let Some(comments) = self.review.get_mut(&key) {
            comments.retain(|comment| comment.id != id);
            if comments.is_empty() {
                self.review_open = false;
            }
        }
        self.save_review(&key);
        self.sync_review_marks(cx);
        cx.notify();
    }

    pub(super) fn toggle_review(&mut self, cx: &mut Context<Self>) {
        self.review_open = !self.review_open;
        cx.notify();
    }

    /// The pending comments of any pull request, saved ones read first.
    fn pending_review(&mut self, key: &str) -> &mut Vec<PendingComment> {
        let store = &self.store;
        self.review.entry(key.to_string()).or_insert_with(|| {
            locked(store)
                .ok()
                .and_then(|store| store.setting(&review::setting_key(key)).ok().flatten())
                .map(|json| review::parse(&json))
                .unwrap_or_default()
        })
    }

    /// What changed in `key`'s comments shows and stays, on either device.
    fn pending_changed(&mut self, key: &str, cx: &mut Context<Self>) -> Vec<PendingComment> {
        self.review_error = None;
        self.review_posted = None;
        self.save_review(key);
        self.sync_review_marks(cx);
        cx.notify();
        self.pending_review(key).clone()
    }

    /// The pending comments a phone reads: the same list the desktop's diff
    /// writes to, whichever pull request the desktop has open.
    pub(crate) fn phone_review(&mut self, key: &str) -> Vec<PendingComment> {
        self.pending_review(key).clone()
    }

    pub(crate) fn phone_review_add(
        &mut self,
        key: &str,
        line: ReviewLine,
        cx: &mut Context<Self>,
    ) -> Vec<PendingComment> {
        let comments = self.pending_review(key);
        let id = review::next_id(comments);
        comments.push(PendingComment {
            id,
            path: line.path,
            line: line.line,
            side: line.side,
            text: line.text,
        });
        self.pending_changed(key, cx)
    }

    pub(crate) fn phone_review_remove(
        &mut self,
        key: &str,
        id: u64,
        cx: &mut Context<Self>,
    ) -> Vec<PendingComment> {
        self.pending_review(key).retain(|comment| comment.id != id);
        self.pending_changed(key, cx)
    }

    /// Claims the review for a phone, so the desktop's button can't post the
    /// same comments while it is on its way. The comments to send.
    pub(crate) fn phone_review_start(
        &mut self,
        key: &str,
        cx: &mut Context<Self>,
    ) -> Result<Vec<PendingComment>, String> {
        if self.review_posting {
            return Err("A review is already being posted".into());
        }
        let comments = self.pending_review(key).clone();
        if comments.is_empty() {
            return Err("There is nothing to submit".into());
        }
        self.review_posting = true;
        self.review_error = None;
        cx.notify();
        Ok(comments)
    }

    /// A phone's review ended. Comments written meanwhile stay pending, and so
    /// does everything when it failed. What is still pending.
    pub(crate) fn phone_review_finished(
        &mut self,
        key: &str,
        sent: &[PendingComment],
        posted: bool,
        cx: &mut Context<Self>,
    ) -> Vec<PendingComment> {
        self.review_posting = false;
        if posted {
            self.pending_review(key)
                .retain(|comment| !sent.iter().any(|gone| gone.id == comment.id));
            self.review_open = false;
            self.phone_discussed(key, cx);
            return self.pending_changed(key, cx);
        }
        cx.notify();
        self.pending_review(key).clone()
    }

    /// Posts the pending comments as one review that only comments.
    pub(super) fn submit_review(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.item() else {
            return;
        };
        let comments = self.review_comments().to_vec();
        if self.review_posting || comments.is_empty() {
            return;
        }
        self.review_posting = true;
        self.review_error = None;
        let key = item.key();
        let service = self.service.clone();
        let task = cx.background_executor().spawn({
            let comments = comments.clone();
            async move { service.submit_review(&item, "", &comments) }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.review_posting = false;
                match result {
                    Ok(url) => {
                        let sent: Vec<u64> = comments.iter().map(|comment| comment.id).collect();
                        if let Some(pending) = this.review.get_mut(&key) {
                            pending.retain(|comment| !sent.contains(&comment.id));
                        }
                        this.save_review(&key);
                        this.sync_review_marks(cx);
                        this.review_open = false;
                        if this.selected.as_ref() == Some(&key) {
                            this.review_posted = Some(url);
                        }
                    }
                    Err(error) => this.review_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

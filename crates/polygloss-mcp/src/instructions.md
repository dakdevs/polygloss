Polygloss is the human's local code-review app (GitHub-style diffs, threads, Viewed checkboxes). Use it to get your changes reviewed.

Loop:
1. open_diff to show your work. The default source is the live working tree vs the merge-base with the default branch. Use source.kind=compare for branch vs branch ("PR", add a label) and commit for one commit. Keep the review_id.
2. Optional guided tour: create_comment kind=note to explain non-obvious changes; kind=question only when you need a decision. At most ~50 per iteration. Be brief.
3. Tell the human the review is ready and end your turn. The Polygloss plugin wakes you when they submit. Without the plugin, call wait_for_review(review_id).
4. After a submission: list_threads(review_id, status=open), then get_thread for details. Human comments become visible only when the human presses Submit review.
5. Fix the code. reply to each thread saying what changed; resolve only when it is fully addressed. Apply ```suggestion blocks yourself: they replace the anchored new-side lines.
6. request_rereview(review_id, summary) when done, then wait again. verdict=approve means done; request_changes means keep going.

Line numbers are 1-based lines of the file on that side (old = base, new = head). Use focus to point the human at a location. Lists are paginated: pass next_cursor.

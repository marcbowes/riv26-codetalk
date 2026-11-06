;;; e2e.el --- Run generated steps against a real cluster, then dump the panes  -*- lexical-binding: t; -*-
;;
;; Used by stage/test/e2e.py: DAT401_E2E_CLUSTER=<id> DAT401_E2E_STEPS=1-27
;; DAT401_E2E_OUT=<dir> emacs -nw --eval '(load ".../e2e.el")'

(defun dat401-e2e--log (fmt &rest args)
  "Append a line to log.txt in the output directory (where a stall shows)."
  (when-let* ((out (getenv "DAT401_E2E_OUT")))
    (write-region (concat (apply #'format fmt args) "\n") nil
                  (expand-file-name "log.txt" out) t 'silent)))

(dat401-e2e--log "e2e.el loading")
(require 'dat401-stage)
(dat401-e2e--log "dat401-stage loaded (terminal %s)" dat401-terminal)

(defun dat401-e2e--wait-text (pane regexp timeout)
  (dat401--wait-for (lambda () (string-match-p regexp (dat401--pane-text pane))) timeout))

(defun dat401-e2e ()
  (dat401-e2e--log "timer fired")
  (load (expand-file-name "stage/dat401-talk.el" dat401-repo) nil t)
  (dat401-e2e--log "talk loaded: %d steps" (length demo-it--steps))
  (let* ((spec (getenv "DAT401_E2E_STEPS"))
         (range (and spec (> (length spec) 0) (split-string spec "-")))
         (from (if range (string-to-number (car range)) 1))
         ;; By default: up to DAT401_E2E_UNTIL (a talk step's title; default step
         ;; 10, so the intro through step 8).
         (to (if range (string-to-number (cadr range))
               (1- (cdr (assoc (or (getenv "DAT401_E2E_UNTIL") "Step 10 · Change data capture")
                               dat401--steps-index)))))
         (out (getenv "DAT401_E2E_OUT")))
    ;; DAT401_E2E_STREAM: the test's own Kinesis stream, not the talk's.
    (when-let* ((stream (getenv "DAT401_E2E_STREAM")))
      (setq demo-it--steps
            (mapcar (lambda (step)
                      (car (read-from-string
                            (replace-regexp-in-string "--stream-name dat401-cdc\\b"
                                                      (concat "--stream-name " stream)
                                                      (prin1-to-string step) t t))))
                    demo-it--steps)))
    (setq dat401-typing-speed 0.05
          ;; No real agent in tests: `cat' shows what's typed, and would echo it
          ;; back (a second copy) if an Enter were ever sent.
          dat401-agent-command "cat"
          dat401-agent-ready ".")
    (dat401-stage-mode 1)
    (dat401-e2e--log "laying out pane A")
    (dat401-layout 'a)
    (dat401-e2e--log "pane A up")
    ;; DAT401_E2E_JUMP="Step 7 · ...": what C-F12 does, fast-forward there
    ;; from the start, then run from that step as usual.
    (when-let* ((jump (getenv "DAT401_E2E_JUMP"))
                (target (cdr (assoc jump dat401--steps-index))))
      (dat401-e2e--log "fast-forwarding 2-%d" (1- target))
      (unless (dat401--fast-forward 2 (1- target))
        (error "fast-forward stopped at step %d" (1+ demo-it--step)))
      (setq from target)
      (dat401-e2e--log "fast-forwarded; running from %d" from))
    (cl-loop for n from from to to do
             (let ((step (nth (1- n) demo-it--steps)))
               (with-temp-file (expand-file-name "progress.txt" out)
                 (insert (format "%d %S\n" n step)))
               (when (and (string-match-p "dat401-quit-agent" (format "%S" step))
                          (get-buffer (dat401--buffer-name 'claude)))
                 (with-temp-file (expand-file-name "agent.txt" out)
                   (insert (dat401--pane-text 'claude))))
               (demo-it--execute-step step)
               ;; What's highlighted in pane A after this step.
               (when (get-buffer (dat401--buffer-name 'a))
                 (let ((marked (with-current-buffer (dat401--buffer-name 'a)
                                 (mapcar (lambda (o) (buffer-substring-no-properties
                                                      (overlay-start o) (overlay-end o)))
                                         dat401--mark-overlays))))
                   (write-region (format "%d %S\n" n marked) nil
                                 (expand-file-name "marks.txt" out) t 'silent)))
               ;; The CDC viewer needs a moment to attach (it reads from the
               ;; latest position) before anything is inserted.
               (when (string-match-p "events/target/release/events" (format "%S" step))
                 (dat401-e2e--wait-text 'events "Ctrl-C to stop" 30)
                 (sit-for 3))
               ;; What a presenter does by eye: wait for the command to finish.
               (cond
                ((string-match-p "05-load-sample-data" (format "%S" step))
                 (dat401-e2e--wait-text 'shell "Done in" 180))
                (t
                 (dolist (p '(a b))
                   (when (get-buffer (dat401--buffer-name p))
                     (dat401--wait-for (lambda () (dat401--at-prompt-p p)) 60)))))
               (sit-for 0.5)))
    ;; The CDC viewer takes a moment to show the comment.
    (when (get-buffer (dat401--buffer-name 'events))
      (dat401-e2e--wait-text 'events "Hello from the stage" 30))
    (dolist (p '(a b shell events))
      (when (get-buffer (dat401--buffer-name p))
        (with-temp-file (expand-file-name (format "pane-%s.txt" p) out)
          (insert (dat401--pane-text p)))))
    (when (buffer-live-p (car dat401--views))
      (with-temp-file (expand-file-name "code.txt" out)
        (insert (with-current-buffer (car dat401--views) (buffer-string)))))
    (with-temp-file (expand-file-name "copies.txt" out) (insert (format "%d\n" dat401--copies)))
    (with-temp-file (expand-file-name "recalls.txt" out)
      (insert (format "%d %d\n" (car dat401--recalls) (cdr dat401--recalls))))
    (with-temp-file (expand-file-name "done.txt" out) (insert "ok\n"))
    (kill-emacs 0)))

(run-at-time 1 nil
             (lambda ()
               (condition-case err
                   (dat401-e2e)
                 (error
                  (with-temp-file (expand-file-name "error.txt" (getenv "DAT401_E2E_OUT"))
                    (insert (format "%S\n" err)))
                  (kill-emacs 1)))))

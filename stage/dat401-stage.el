;;; dat401-stage.el --- Typed-looking terminal demos for DAT401 (re:Invent 2026)  -*- lexical-binding: t; -*-

;; The runtime for the talk's demo-it script (dat401-talk.el, generated from
;; TALK.md by generate.py). The aim is a demo that looks live, not a
;; slideshow: no titles, no banners, just terminals and files. It provides:
;;
;;   - panes: vterm buffers for psql A, psql B, a shell, the events viewer and
;;     the load generator, all started in the repo root;
;;   - layouts: one pane, two side by side, or one over another;
;;   - typing at a person's pace and rhythm: an uneven tempo, quick bursts and
;;     slower words, gaps between words and after punctuation, reaching for
;;     shifted keys, the odd pause to think;
;;   - copying, acted out: an id a command needs (<cluster-id>, <job_id>) is
;;     copied from the output above in vterm's copy mode (up, along a word at
;;     a time, select, copy) and pasted, never typed; whole blocks can be
;;     pasted too;
;;   - highlights on terminal output (a JSON field's value) that survive the
;;     terminal redrawing, until cleared;
;;   - files on screen: code narrowed to an excerpt from TALK.md, with the
;;     excerpt's lines highlighted, and the step 4 prompt.
;;
;; Keys while the demo runs: F12 or PgDn (clickers send PgDn) runs the next
;; step; C-u N F12 jumps to step N; M-x dat401-goto jumps to a talk step by
;; name. Any key while typing finishes the current command instantly.

;;; Code:

(require 'cl-lib)
(require 'subr-x)
(require 'demo-it)
(require 'vterm nil t)

(defgroup dat401 nil
  "DAT401 stage demo."
  :group 'tools)

(defcustom dat401-repo
  (file-name-directory (directory-file-name
                        (file-name-directory (or load-file-name buffer-file-name default-directory))))
  "The riv26-codetalk checkout; panes start here."
  :type 'directory)

(defcustom dat401-aws-profile "bowes" "AWS_PROFILE for every pane." :type 'string)
(defcustom dat401-aws-region "us-west-2" "AWS_REGION for every pane." :type 'string)
(defcustom dat401-text-scale 2 "Text scale for panes and files." :type 'integer)

(defcustom dat401-typing-speed 1.0
  "Multiplier on typing delays: 1.0 is a person at a keyboard (about 7
characters a second, unevenly), 0.5 twice as fast, 0 instant."
  :type 'number)

(defcustom dat401-wait-timeout 180
  "Seconds to wait for psql to come back before typing the next statement."
  :type 'integer)

(defvar dat401--vars (make-hash-table :test 'equal)
  "Placeholder values, e.g. \"cluster-endpoint\".")

(defvar dat401--section "" "The current talk step (for `dat401-goto').")
(defvar dat401--layout nil "The panes on screen, e.g. (a b), or (view) for a file.")
(defvar dat401--steps-index nil "Alist of (TITLE . demo-it step number), from dat401-talk.el.")

;;;; Panes

(defconst dat401--pane-titles
  '((a . "psql A") (b . "psql B") (shell . "shell") (events . "events") (load . "load")))

(defun dat401--buffer-name (pane)
  (format "*%s*" (alist-get pane dat401--pane-titles)))

(defun dat401--env ()
  "Environment for new panes.
AWS settings always; psql settings once the cluster's endpoint is known."
  (append
   (list (concat "AWS_PROFILE=" dat401-aws-profile)
         (concat "AWS_REGION=" dat401-aws-region)
         ;; No pager: EXPLAIN plans should just print.
         "PSQL_PAGER=cat" "PAGER=cat"
         ;; No zsh "%" partial-line marker when vterm resizes a new pane.
         "PROMPT_EOL_MARK=")
   (when-let* ((endpoint (gethash "cluster-endpoint" dat401--vars)))
     (list (concat "PGHOST=" endpoint)
           "PGUSER=admin" "PGDATABASE=postgres" "PGSSLMODE=require"))
   process-environment))

(defvar-local dat401--this-pane nil "Which pane this buffer is.")
(defvar-local dat401--fresh nil "Non-nil until a new pane's shell has drawn its first prompt.")

(defun dat401--pane (pane)
  "PANE's vterm buffer, created (in the repo root) on first use."
  (let ((name (dat401--buffer-name pane)))
    (or (get-buffer name)
        (let ((default-directory (file-name-as-directory dat401-repo))
              (process-environment (dat401--env)))
          (save-window-excursion
            (let ((buf (vterm name)))
              (with-current-buffer buf
                (setq dat401--this-pane pane
                      dat401--fresh t)
                (setq-local header-line-format nil)
                (text-scale-set dat401-text-scale))
              buf))))))

;;;; Layout

(defun dat401--normalize (panes)
  (cond ((memq 'load panes) '(load))
        ((memq 'b panes) '(a b))
        ((memq 'events panes) '(a events))
        ((memq 'shell panes) '(a shell))
        (t '(a))))

(defun dat401-layout (&rest panes)
  "Show PANES: one of (a), (a b), (a shell), (a events) or (load)."
  (let ((panes (dat401--normalize panes)))
    (delete-other-windows)
    (switch-to-buffer (dat401--pane (car panes)))
    (pcase panes
      (`(a b) (set-window-buffer (split-window-right) (dat401--pane 'b)))
      (`(a events) (set-window-buffer (split-window-right) (dat401--pane 'events)))
      (`(a shell) (set-window-buffer (split-window-below (- (floor (window-total-height) 3)))
                                     (dat401--pane 'shell))))
    (setq dat401--layout panes)
    (dat401--tidy panes)))

(defun dat401--tidy (panes)
  "Clear the new PANES once their shell has drawn its first prompt at the size
they're shown at. A pane is created at one size and shown at another, and
the shell's redraw leaves stray lines. Panes with history are left alone."
  (let ((fresh (cl-remove-if-not
                (lambda (p) (buffer-local-value 'dat401--fresh (dat401--pane p))) panes)))
    (when fresh
      (redisplay t)
      (dolist (p fresh)
        (dat401--wait-for (lambda () (dat401--shell-prompt-p p)) 5))
      (sit-for 0.4)
      (dolist (p fresh)
        (with-current-buffer (dat401--pane p)
          (setq dat401--fresh nil)
          (vterm-clear)))
      (sit-for 0.2))))

(defun dat401--show (pane)
  "Make sure PANE is on screen, switching layout if needed."
  (unless (memq pane dat401--layout)
    (apply #'dat401-layout (cons pane (remq 'view dat401--layout)))))

(defun dat401-open (pane)
  "Open PANE next to what's on screen (e.g. pane B beside A)."
  (dat401--show pane)
  (select-window (get-buffer-window (dat401--pane pane))))

(defun dat401-close (pane)
  "Take PANE off screen (its session keeps running), e.g. the shell after
the loader, so what's left gets the whole screen."
  (when (memq pane dat401--layout)
    (apply #'dat401-layout (or (remq pane dat401--layout) '(a)))))

(defun dat401-section (title)
  "Note the current talk step (nothing on screen: it should look live)."
  (setq dat401--section title))

;;;; Files on screen

(defface dat401-code-mark
  '((((background dark)) :extend t :background "#2e3b52")
    (t :extend t :background "#fff1c2"))
  "The excerpt's lines.")

(defface dat401-code-dim '((t :inherit shadow))
  "Lines of the file between the excerpt's.")

(defvar dat401--views nil "Buffers made to show files, killed when replaced.")

(defun dat401--mode-for (path)
  "Set the major mode for PATH without its hooks: no LSP, no linters on stage."
  (let* ((m (assoc-default path auto-mode-alist #'string-match))
         (m (if (consp m) (car m) m))
         (modes (delq nil (list (and (functionp m) m)
                                (and (string-match-p "\\.[jt]sx?\\'" path) #'js-mode)
                                #'text-mode))))
    (cl-loop for mode in modes
             until (ignore-errors (delay-mode-hooks (funcall mode)) t))
    (font-lock-mode 1)
    (font-lock-ensure)))

(defun dat401--view (name fill)
  "Show a buffer called NAME full screen, like a file just opened. FILL is
called in it to insert and decorate the contents."
  (mapc (lambda (b) (when (buffer-live-p b) (kill-buffer b))) dat401--views)
  (dat401-unhighlight)
  (let ((buf (generate-new-buffer name)))
    (setq dat401--views (list buf))
    (with-current-buffer buf
      (funcall fill)
      (setq-local cursor-type nil header-line-format nil)
      (text-scale-set dat401-text-scale)
      (set-buffer-modified-p nil)
      (read-only-mode 1)
      (goto-char (point-min)))
    (delete-other-windows)
    (switch-to-buffer buf)
    (set-window-start nil (point-min))
    (setq dat401--layout '(view))))

(defun dat401--line-pos (n)
  "Position of the start of line N (1-based) in the current (widened) buffer."
  (save-excursion (goto-char (point-min)) (forward-line (1- n)) (point)))

(defun dat401-code (file ranges)
  "Open FILE (relative to the repo) full screen, narrowed to RANGES' span.
RANGES are (FIRST . LAST) line numbers: the excerpt TALK.md shows. Those
lines are highlighted; the lines between them are dimmed."
  (let ((path (expand-file-name file dat401-repo)))
    (dat401--view
     (file-name-nondirectory file)
     (lambda ()
       (insert-file-contents path)
       (dat401--mode-for path)
       (cl-loop for (a . b) in ranges
                for prev = nil then last
                for last = b
                do (overlay-put (make-overlay (dat401--line-pos a) (dat401--line-pos (1+ b)))
                                'face 'dat401-code-mark)
                when (and prev (> a (1+ prev)))
                do (overlay-put (make-overlay (dat401--line-pos (1+ prev)) (dat401--line-pos a))
                                'face 'dat401-code-dim))
       (narrow-to-region (dat401--line-pos (caar ranges))
                         (dat401--line-pos (1+ (cdar (last ranges)))))
       (setq-local display-line-numbers-widen t)
       (display-line-numbers-mode 1)))))

(defun dat401-prompt (text)
  "Open an AI prompt full screen, as if it were a file (prompt.md)."
  (let ((text (dat401--resolve-static text)))
    (dat401--view
     "prompt.md"
     (lambda ()
       (insert text "\n")
       (dat401--mode-for "prompt.md")
       (setq-local word-wrap t truncate-lines nil)
       (visual-line-mode 1)))))

;;;; Placeholders

(defun dat401--placeholders (text)
  "The <names> in TEXT, in order."
  (let ((pos 0) out)
    (while (string-match "<\\([A-Za-z][A-Za-z0-9_-]*\\)>" text pos)
      (push (match-string 1 text) out)
      (setq pos (match-end 0)))
    (nreverse out)))

(defun dat401--segments (line)
  "LINE as a list of (TEXT . nil) to type and (NAME . t) placeholders to paste."
  (let ((pos 0) out)
    (while (string-match "<\\([A-Za-z][A-Za-z0-9_-]*\\)>" line pos)
      (when (> (match-beginning 0) pos)
        (push (cons (substring line pos (match-beginning 0)) nil) out))
      (push (cons (match-string 1 line) t) out)
      (setq pos (match-end 0)))
    (when (< pos (length line))
      (push (cons (substring line pos) nil) out))
    (nreverse out)))

(defun dat401--last-job-id (text)
  "The most recent job id in psql output TEXT (the row under a job_id header)."
  (let ((pos 0) found)
    (while (string-match "job_id[ \t]*\n[-+ \t]+\n[ \t]*\\([a-z0-9]\\{26\\}\\)\\b" text pos)
      (setq found (match-string 1 text) pos (match-end 0)))
    found))

(defun dat401--endpoints (text)
  "Every cluster endpoint in `create-cluster' output TEXT, oldest first."
  (let ((pos 0) out)
    (while (string-match "\"endpoint\": \"\\([a-z0-9]+\\.dsql\\.[a-z0-9-]+\\.on\\.aws\\)\"" text pos)
      (push (match-string 1 text) out)
      (setq pos (match-end 0)))
    (nreverse out)))

(defvar dat401--stale-endpoints nil
  "Endpoints on screen before the latest `create-cluster': not the new cluster's.")

(defun dat401--new-endpoint ()
  "The endpoint the latest `create-cluster' printed, if it has yet."
  (cl-some (lambda (p)
             (when (get-buffer (dat401--buffer-name p))
               (car (last (cl-set-difference (dat401--endpoints (dat401--pane-text p))
                                             dat401--stale-endpoints :test #'equal)))))
           '(a b shell)))

(defun dat401--value (name pane)
  "Value for <NAME>, typing into PANE."
  (cond
   ((equal name "job_id")
    (or (dat401--wait-for (lambda () (dat401--last-job-id (dat401--pane-text pane))) 30)
        (read-string "job_id: ")))
   ((equal name "cluster-endpoint")
    (or (gethash name dat401--vars)
        (puthash name (or (dat401--wait-for #'dat401--new-endpoint 30)
                          (string-trim (read-string "cluster endpoint: ")))
                 dat401--vars)))
   ((equal name "cluster-id")
    (car (split-string (dat401--value "cluster-endpoint" pane) "\\.")))
   (t
    (or (gethash name dat401--vars)
        (puthash name (string-trim (read-string (format "%s: " name))) dat401--vars)))))

(defun dat401--resolve (text pane)
  (dolist (name (dat401--placeholders text) text)
    (setq text (replace-regexp-in-string (regexp-quote (format "<%s>" name))
                                         (dat401--value name pane) text t t))))

(defun dat401--resolve-static (text)
  "Fill in placeholders we already know (for display only)."
  (dolist (name (dat401--placeholders text) text)
    (let ((v (if (equal name "cluster-id")
                 (when-let* ((endpoint (gethash "cluster-endpoint" dat401--vars)))
                   (car (split-string endpoint "\\.")))
               (gethash name dat401--vars))))
      (when v (setq text (replace-regexp-in-string (regexp-quote (format "<%s>" name)) v text t t))))))

;;;; Highlights on terminal output

(defface dat401-mark '((t :background "#ffd75f" :foreground "black" :weight bold))
  "Highlighted text in a pane.")

(defvar dat401--marks nil
  "Highlights on screen: (PANE REGEXP). Each highlights group 1 of the last
match of REGEXP in PANE, and is re-applied whenever the terminal redraws.")

(defvar-local dat401--mark-overlays nil)

(defun dat401--apply-marks (pane)
  "(Re)draw the highlights in PANE."
  (when-let* ((buf (get-buffer (dat401--buffer-name pane))))
    (with-current-buffer buf
      (mapc #'delete-overlay dat401--mark-overlays)
      (setq dat401--mark-overlays nil)
      (save-excursion
        (save-match-data
          (dolist (m dat401--marks)
            (when (eq (car m) pane)
              (goto-char (point-max))
              (when (re-search-backward (cadr m) nil t)
                (let ((ov (make-overlay (match-beginning 1) (match-end 1))))
                  (overlay-put ov 'face 'dat401-mark)
                  (overlay-put ov 'priority 100)
                  (push ov dat401--mark-overlays))))))))))

(defun dat401--after-redraw (&rest _)
  "vterm redraws by replacing lines, which takes overlays with them."
  (when (and dat401--marks dat401--this-pane)
    (dat401--apply-marks dat401--this-pane)))

(with-eval-after-load 'vterm
  (advice-add 'vterm--redraw :after #'dat401--after-redraw))

(defun dat401--mark (pane regexp)
  (push (list pane regexp) dat401--marks)
  (dat401--apply-marks pane))

(defconst dat401--named-marks
  '(("dpu" . "\\(Total: [0-9.]+ DPU\\)")
    ("index" . "\\(Index\\(?: Only\\)? Scan\\(?: Backward\\)? using [a-z0-9_]+\\)"))
  "Things to highlight by name. Anything else is a JSON field.")

(defun dat401-highlight (pane what)
  "Highlight WHAT in PANE's latest output: an EXPLAIN's DPU total (\"dpu\")
or the index scan in its plan (\"index\"), or the value of a JSON field
\(\"identifier\")."
  (dat401--mark pane (or (cdr (assoc what dat401--named-marks))
                         (format "\"%s\": \"\\([^\"\n]+\\)\"" (regexp-quote what)))))

(defun dat401-unhighlight ()
  "Clear every highlight."
  (let ((panes (delete-dups (mapcar #'car dat401--marks))))
    (setq dat401--marks nil)
    (mapc #'dat401--apply-marks panes)))

;;;; Typing

(defun dat401--rand (lo hi) (+ lo (random (1+ (- hi lo)))))

(defun dat401--gauss ()
  "Roughly a standard normal sample (the sum of 12 uniforms, minus 6)."
  (let ((s 0.0))
    (dotimes (_ 12) (cl-incf s (/ (random 10000) 10000.0)))
    (- s 6.0)))

(defun dat401--jitter (ms spread)
  "MS, scaled by a log-normal factor: usually within a factor of e^SPREAD."
  (* ms (exp (* spread (dat401--gauss)))))

(defun dat401--pause (ms spread)
  "A pause of about MS milliseconds, in seconds, at the typing speed."
  (* (/ (dat401--jitter ms spread) 1000.0) dat401-typing-speed))

(defconst dat401--shifted (string-to-list "_()\"'<>{}*$:|&!?%+~^@#")
  "Characters that need Shift (on a US keyboard), or a reach.")

(defun dat401--delays (line)
  "A list of (STRING . SECONDS): LINE typed by a person. The line has its own
tempo; each word is either a quick burst or typed steadily; there's a gap
between words and a longer one after punctuation; Shift costs a moment;
now and then there's a pause to think before a word, or a hiccup mid-word."
  (let ((tempo (dat401--jitter 1.0 0.18))
        (out '()) (prev nil) (burst nil))
    (dolist (ch (string-to-list line))
      (when (or (null prev) (memq prev '(?\s ?\( ?. ?, ?= ?\;)))
        (setq burst (< (random 100) 20)))
      (let ((ms (if burst (dat401--jitter 55 0.25) (dat401--jitter 95 0.4))))
        (when (eq prev ?\s)
          (cl-incf ms (dat401--jitter 150 0.45))
          (when (< (random 100) 8)                       ; thinking about the next word
            (cl-incf ms (dat401--jitter 700 0.45))))
        (when (memq prev '(?, ?\; ?\( ?\) ?. ?= ?:))
          (cl-incf ms (dat401--jitter 160 0.5)))
        (when (or (memq ch dat401--shifted)              ; Shift, or a reach
                  (and (<= ?A ch ?Z) (not (and prev (<= ?A prev ?Z)))))
          (cl-incf ms (dat401--jitter 60 0.4)))
        (when (< (random 1000) 15)                       ; a hiccup
          (cl-incf ms (dat401--jitter 300 0.4)))
        (push (cons (char-to-string ch) (* (/ (* ms tempo) 1000.0) dat401-typing-speed)) out)
        (setq prev ch)))
    (nreverse out)))

(defvar dat401--hurry nil "Non-nil: finish the current command instantly.")

(defun dat401--sleep (secs)
  "Wait SECS, redisplaying. A key press sets `dat401--hurry' (and is swallowed)."
  (if (or dat401--hurry (<= secs 0))
      (redisplay)
    (unless (sit-for secs)
      (setq dat401--hurry t)
      (read-event nil nil 0.01))))

(defun dat401--pane-text (pane)
  "PANE's text, with lines the terminal wrapped joined back up."
  (with-current-buffer (dat401--pane pane)
    (substring-no-properties (filter-buffer-substring (point-min) (point-max)))))

(defun dat401--wait-for (pred timeout)
  "Poll PRED until it returns non-nil or TIMEOUT seconds pass."
  (let ((deadline (+ (float-time) timeout)) v)
    (while (and (not (setq v (funcall pred))) (< (float-time) deadline))
      (sit-for 0.1))
    v))

(defun dat401--at-prompt-p (pane)
  "Is PANE showing a fresh psql or shell prompt?"
  (let ((tail (string-trim-right (dat401--pane-text pane))))
    (string-match-p "\\(=[*!]?[>#]\\|[$%#>❯]\\)\\'" tail)))

(defun dat401--shell-prompt-p (pane)
  "Is PANE at a shell prompt (as opposed to psql)?"
  (let ((tail (string-trim-right (dat401--pane-text pane))))
    (string-match-p "[$%❯]\\'" tail)))

(defun dat401--send (pane string &optional paste)
  (with-current-buffer (dat401--pane pane) (vterm-send-string string paste)))

(defun dat401--paste-value (pane before value)
  "Paste VALUE into PANE. While highlights are on, highlight it too (found by
BEFORE, the text typed ahead of it on this line)."
  (dat401--send pane value)
  (when dat401--marks
    (let ((tail (substring before (max 0 (- (length before) 24)))))
      (dat401--mark pane (concat (regexp-quote tail) "\\(" (regexp-quote value) "\\)"))))
  (dat401--sleep (dat401--pause 250 0.35)))

;;;; Copying from the screen (acted out)

(defconst dat401--copy-context
  '(("cluster-id" . "\"identifier\": \"")
    ("cluster-endpoint" . "\"endpoint\": \""))
  "Where on screen a value is copied from: the text just before it. A value
with no entry is copied from wherever it last appears.")

(defvar dat401--copies 0 "How many copies have been acted out (for tests).")

(defun dat401--id-like-p (value)
  "Is VALUE something nobody types by hand (a job id, a cluster id)?"
  (string-match-p "\\`[a-z0-9]\\{20,\\}\\'" value))

(defun dat401--find-on-screen (buf value context)
  "Start of VALUE in BUF's output above the current input line, or nil."
  (with-current-buffer buf
    (save-excursion
      (goto-char (point-max))
      (skip-chars-backward " \t\n")
      (forward-line 0)
      (when (search-backward (concat (or context "") value) nil t)
        (+ (point) (length (or context "")))))))

(defun dat401--word-starts (from to)
  "Where `w' (forward to word start) would stop between FROM and TO."
  (save-excursion
    (goto-char from)
    (let (out)
      (while (re-search-forward "\\(\\w\\|\\s_\\)+" to t)
        (when (< (match-beginning 0) to) (push (match-beginning 0) out)))
      (nreverse out))))

(defun dat401--copy-from-screen (pane value &optional context)
  "Act out copying VALUE from PANE's earlier output, the way you would in
vterm's copy mode: up to its line (a couple of taps, then key repeat), along
it a word at a time, set the mark, select it, a beat to check, copy, and back
to the prompt. Nothing touches the clipboard: the caller pastes VALUE from a
variable. Returns non-nil if VALUE was on screen to copy."
  (let* ((buf (dat401--pane pane))
         (win (get-buffer-window buf))
         (start (and win (not dat401--hurry) (dat401--find-on-screen buf value context))))
    (when start
      (cl-incf dat401--copies)
      (with-selected-window win
        (with-current-buffer buf
          (dat401--sleep (dat401--pause 400 0.3))
          (vterm-copy-mode 1)
          (unwind-protect
              (let ((target (line-number-at-pos start)) (taps 0))
                (dat401--sleep (dat401--pause 250 0.3))
                (while (and (> (line-number-at-pos) target) (not dat401--hurry))
                  (forward-line -1)
                  (cl-incf taps)
                  (dat401--sleep (if (< taps 3) (dat401--pause 160 0.3) (dat401--pause 45 0.2))))
                (forward-line 0)
                (dat401--sleep (dat401--pause 200 0.3))
                (dolist (p (append (dat401--word-starts (point) start) (list start)))
                  (unless (or dat401--hurry (= p (point)))
                    (goto-char p)
                    (dat401--sleep (dat401--pause 170 0.35))))
                (goto-char start)
                (push-mark start t t)
                (dat401--sleep (dat401--pause 280 0.3))
                (goto-char (+ start (length value)))
                (dat401--sleep (dat401--pause 550 0.3))
                (deactivate-mark))
            (vterm-copy-mode -1))))
      (dat401--sleep (dat401--pause 300 0.3))
      t)))

(defun dat401--insert-value (pane name before value)
  "Put placeholder NAME's VALUE into PANE's command line, as a person would:
copied from the screen if it's there, pasted if it's an id, otherwise typed."
  (cond
   ((dat401--copy-from-screen pane value (cdr (assoc name dat401--copy-context)))
    (dat401--paste-value pane before value))
   ((dat401--id-like-p value)
    (dat401--sleep (dat401--pause 450 0.35))
    (dat401--paste-value pane before value))
   (t
    (dolist (cd (dat401--delays value))
      (dat401--send pane (car cd))
      (dat401--sleep (cdr cd))))))

(defun dat401--type-line (pane line)
  "Type LINE into PANE like a person, copying in its placeholders, then Enter."
  (let ((typed ""))
    (dat401--sleep (dat401--pause 550 0.35))
    (dolist (seg (dat401--segments line))
      (if (cdr seg)
          (let ((v (dat401--value (car seg) pane)))
            (dat401--insert-value pane (car seg) typed v)
            (setq typed (concat typed v)))
        (dolist (cd (dat401--delays (car seg)))
          (dat401--send pane (car cd))
          (dat401--sleep (cdr cd)))
        (setq typed (concat typed (car seg)))))
    (dat401--sleep (dat401--pause 380 0.4))
    (with-current-buffer (dat401--pane pane) (vterm-send-return))))

(defun dat401--statement-end-p (line)
  "Does LINE finish something psql will run (a ;, or a backslash command)?"
  (let ((l (string-trim line)))
    (or (string-suffix-p ";" l) (string-prefix-p "\\" l) (string-match-p "\\\\gset\\'" l))))

(defun dat401--new-cluster-coming ()
  "A `create-cluster' is about to run: its endpoint is whichever one appears next."
  (setq dat401--stale-endpoints
        (apply #'append (mapcar (lambda (p)
                                  (when (get-buffer (dat401--buffer-name p))
                                    (dat401--endpoints (dat401--pane-text p))))
                                '(a b shell))))
  (remhash "cluster-endpoint" dat401--vars))

(defun dat401--wait-prompt (pane)
  (sit-for 0.3)
  (dat401--wait-for (lambda () (dat401--at-prompt-p pane)) dat401-wait-timeout))

(defun dat401--type (pane text)
  "Type TEXT into PANE line by line. Between lines, wait for a shell command
or a psql statement to finish, so nothing is typed over its output. Never
wait after the last line: it may start something that keeps running."
  (let ((lines (split-string text "\n" t)))
    (while lines
      (let* ((line (pop lines))
             (shell (dat401--shell-prompt-p pane)))
        (when (string-match-p "aws dsql create-cluster" line)
          (dat401--new-cluster-coming))
        (dat401--type-line pane line)
        (when (and lines (or shell (dat401--statement-end-p line)))
          (dat401--wait-prompt pane))))))

(defun dat401--start (panes)
  (setq dat401--hurry nil)
  ;; Every target pane on screen first, so the layout doesn't change halfway.
  (dolist (p panes) (dat401--show p)))

(defun dat401-run (panes text)
  "Type TEXT into each of PANES (a pane symbol or a list), one after the other.
Highlights clear first, unless TEXT pastes a value (step 1's PGHOST export
pastes the highlighted identifier, and it's highlighted too)."
  (let ((panes (if (listp panes) panes (list panes))))
    (unless (dat401--placeholders text)
      (dat401-unhighlight))
    (dat401--start panes)
    (dolist (p panes)
      (select-window (get-buffer-window (dat401--pane p)))
      (dat401--type p text)
      (setq dat401--hurry nil))))

(defun dat401-paste (pane text)
  "Paste TEXT (placeholders filled in) into PANE in one go, then Enter, the
way you'd paste a block of exports. At a shell prompt it's a bracketed
paste, so the shell shows it as pasted and runs it on Enter."
  (dat401--start (list pane))
  (select-window (get-buffer-window (dat401--pane pane)))
  (let ((text (string-trim-right (dat401--resolve text pane))))
    (dat401--sleep (dat401--pause 600 0.3))
    (if (dat401--shell-prompt-p pane)
        (dat401--send pane text t)
      (dat401--send pane text))
    (dat401--sleep (dat401--pause 500 0.3))
    (with-current-buffer (dat401--pane pane) (vterm-send-return))
    (dat401--wait-prompt pane)))

;;;; Starting, jumping, keys

(defvar dat401-stage-map
  (let ((map (make-sparse-keymap)))
    (define-key map (kbd "<next>") #'demo-it-step)
    (define-key map (kbd "<f12>") #'demo-it-step)
    (define-key map (kbd "C-<f12>") #'dat401-goto)
    map)
  "Keys that win over evil and vterm while the demo runs.")

(define-minor-mode dat401-stage-mode
  "DAT401 demo keys: F12 / PgDn next step, C-F12 jump to a talk step."
  :global t :lighter " DAT401")

(add-to-list 'emulation-mode-map-alists `((dat401-stage-mode . ,dat401-stage-map)))

(defun dat401-goto (title)
  "Jump to the first step of talk step TITLE (the next F12 runs it)."
  (interactive (list (completing-read "Talk step: " (mapcar #'car dat401--steps-index) nil t)))
  (let ((n (alist-get title dat401--steps-index nil nil #'equal)))
    (setq demo-it--step (1- n))
    (dat401-section title)
    (message "Next F12 runs: %s" title)))

;;;###autoload
(defun dat401-start ()
  "Load the talk's steps and start: psql A on screen, F12 / PgDn to go."
  (interactive)
  (load (expand-file-name "stage/dat401-talk.el" dat401-repo) nil t)
  (dat401-stage-mode 1)
  (setq demo-it--start-single-window nil)
  (dat401-layout 'a)
  (demo-it-start nil t)
  (message "DAT401: F12 / PgDn runs the next step, C-F12 jumps, any key finishes typing"))

(defun dat401-stop ()
  "Leave the demo (panes stay)."
  (interactive)
  (dat401-stage-mode -1)
  (demo-it-disable-mode))

(defun dat401-reset ()
  "Leave the demo, close every pane (ending their psql sessions) and forget
placeholder values like the cluster endpoint. The next `dat401-start' starts
clean."
  (interactive)
  (ignore-errors (dat401-stop))
  (dolist (name (mapcar (lambda (p) (dat401--buffer-name (car p))) dat401--pane-titles))
    (when-let* ((buf (get-buffer name)))
      (when-let* ((proc (get-buffer-process buf)))
        (set-process-query-on-exit-flag proc nil))
      (kill-buffer buf)))
  (mapc (lambda (b) (when (buffer-live-p b) (kill-buffer b))) dat401--views)
  (clrhash dat401--vars)
  (setq dat401--views nil dat401--marks nil dat401--layout nil dat401--section ""
        dat401--stale-endpoints nil)
  (message "DAT401: reset. M-x dat401-start to begin again"))

(provide 'dat401-stage)
;;; dat401-stage.el ends here

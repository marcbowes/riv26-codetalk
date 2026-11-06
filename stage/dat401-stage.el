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
(require 'ghostel nil t)
(require 'vterm nil t)

(defgroup dat401 nil
  "DAT401 stage demo."
  :group 'tools)

(defcustom dat401-repo
  (file-name-directory (directory-file-name
                        (file-name-directory (or load-file-name buffer-file-name default-directory))))
  "The riv26-codetalk checkout; panes start here."
  :type 'directory)

(defcustom dat401-terminal (if (featurep 'ghostel) 'ghostel 'vterm)
  "The terminal the panes run in: `ghostel' (libghostty, Ghostty's VT engine)
or `vterm' (libvterm)."
  :type '(choice (const ghostel) (const vterm)))

(defcustom dat401-aws-profile "bowes" "AWS_PROFILE for every pane." :type 'string)
(defcustom dat401-aws-region "us-west-2" "AWS_REGION for every pane." :type 'string)
(defcustom dat401-text-scale 2
  "Text scale for files on screen. Terminal panes are never text-scaled: a
scaled vterm can be told the wrong size, which mangles full-screen programs
(the load generator) and can crash Emacs on resize. They get big through
`dat401-big-font' instead."
  :type 'integer)

(defcustom dat401-big-font t
  "Make the whole frame's font big while the demo runs (Doom's
`doom-big-font-mode', or the default face scaled by 1.5)."
  :type 'boolean)

(defvar dat401--saved-height nil "The default face height before the demo, if we changed it.")

(defun dat401--big-font (on)
  (when dat401-big-font
    (cond
     ((fboundp 'doom-big-font-mode) (doom-big-font-mode (if on 1 -1)))
     (on (setq dat401--saved-height (face-attribute 'default :height))
         (set-face-attribute 'default nil :height (round (* 1.5 dat401--saved-height))))
     (dat401--saved-height (set-face-attribute 'default nil :height dat401--saved-height)
                           (setq dat401--saved-height nil)))))

(defcustom dat401-typing-speed 1.0
  "Multiplier on typing delays: 1.0 is a quick typist (about 90-100 words a
minute over a line, unevenly), 0.5 twice as fast, 0 instant."
  :type 'number)

(defcustom dat401-agent-command "claude"
  "What step 4 types to start the AI agent in its frame."
  :type 'string)

(defcustom dat401-agent-typing-speed 0.45
  "Typing delays for the AI prompt, relative to `dat401-typing-speed'.
Prose goes in faster than SQL."
  :type 'number)

(defcustom dat401-agent-ready "shift\\+tab to cycle\\|for shortcuts"
  "Seen in the agent's frame once it's ready for a prompt."
  :type 'regexp)

(defcustom dat401-wait-timeout 180
  "Seconds to wait for psql to come back before typing the next statement."
  :type 'integer)

(defvar dat401--vars (make-hash-table :test 'equal)
  "Placeholder values, e.g. \"cluster-endpoint\".")

(defvar dat401--section "" "The current talk step (for `dat401-goto').")
(defvar dat401--hurry nil "Non-nil: finish the current command instantly.")
(defvar dat401--layout nil "The panes on screen, e.g. (a b), or (view) for a file.")
(defvar dat401--steps-index nil "Alist of (TITLE . demo-it step number), from dat401-talk.el.")

;;;; Terminal backend (ghostel or vterm)

(declare-function ghostel-create "ghostel" (&optional name display identity))
(declare-function ghostel-send-string "ghostel" (string))
(declare-function ghostel-paste-string "ghostel" (string))
(declare-function ghostel-send-key "ghostel" (key-name &optional mods))
(declare-function ghostel-clear "ghostel" ())
(declare-function ghostel-copy-mode "ghostel" (&optional sticky))
(declare-function ghostel-readonly-exit "ghostel" ())
(declare-function vterm "vterm" (&optional arg))
(declare-function vterm-send-string "vterm" (string &optional paste-p))
(declare-function vterm-send-return "vterm" ())
(declare-function vterm-send-key "vterm" (key &optional shift meta ctrl accept-proc-output))
(declare-function vterm-clear "vterm" (&optional arg))
(declare-function vterm-copy-mode "vterm" (&optional arg))

(defun dat401--ghostel-p () (eq dat401-terminal 'ghostel))

(defun dat401--term-new (name)
  "A new terminal buffer called NAME running the shell, not yet displayed."
  (if (dat401--ghostel-p)
      (ghostel-create name)
    (save-window-excursion (vterm name))))

(defun dat401--term-send (string &optional paste)
  "Send STRING to the current buffer's terminal; PASTE: as a bracketed paste."
  (if (dat401--ghostel-p)
      (if paste (ghostel-paste-string string) (ghostel-send-string string))
    (vterm-send-string string paste)))

(defun dat401--term-return ()
  (if (dat401--ghostel-p) (ghostel-send-key "return") (vterm-send-return)))

(defun dat401--term-ctrl (key)
  "Send Ctrl-KEY (a one-letter string)."
  (if (dat401--ghostel-p) (ghostel-send-key key "ctrl") (vterm-send-key key nil nil t)))

(defun dat401--term-clear ()
  (if (dat401--ghostel-p) (ghostel-clear) (vterm-clear)))

(defun dat401--term-copy-mode (on)
  "Enter (ON) or leave the terminal's copy mode, without a word in the echo
area: the audience shouldn't see \"Copy mode\" go by."
  (let ((inhibit-message t))
    (if (dat401--ghostel-p)
        (if on
            (unless (eq (bound-and-true-p ghostel--input-mode) 'copy) (ghostel-copy-mode))
          (when (eq (bound-and-true-p ghostel--input-mode) 'copy) (ghostel-readonly-exit)))
      (vterm-copy-mode (if on 1 -1)))))

(defun dat401--wrap-property ()
  "The text property the terminal puts on newlines it added to wrap a line."
  (if (dat401--ghostel-p) 'ghostel-wrap 'vterm-line-wrap))

(defun dat401--kill-terminal (buf)
  "Kill terminal buffer BUF and its processes, never asking (a question in the
minibuffer stops the demo). Ghostel adds a buffer-local kill check, so the
checks are cleared in BUF itself, and every process it owns is told not to
ask. If killing still fails, BUF is just buried: it's off screen anyway."
  (when (buffer-live-p buf)
    (dolist (proc (process-list))
      (when (eq (process-buffer proc) buf)
        (set-process-query-on-exit-flag proc nil)))
    (with-current-buffer buf
      (setq-local kill-buffer-query-functions nil))
    (condition-case nil
        (let ((kill-buffer-query-functions nil))
          (kill-buffer buf))
      (error (bury-buffer buf)))))

;;;; Panes

(defconst dat401--pane-titles
  '((a . "psql A") (b . "psql B") (shell . "shell") (events . "events") (load . "load")
    (claude . "claude")))

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
   ;; Not "inside" another Claude Code session, even if Emacs was started from one.
   (cl-remove-if (lambda (e) (string-match-p "\\`CLAUDE\\(CODE\\|_CODE_ENTRYPOINT\\)=" e))
                 process-environment)))

(defvar-local dat401--this-pane nil "Which pane this buffer is.")
(defvar-local dat401--fresh nil "Non-nil until a new pane's shell has drawn its first prompt.")

(defun dat401--pane (pane)
  "PANE's vterm buffer, created (in the repo root) on first use."
  (let ((name (dat401--buffer-name pane)))
    (or (get-buffer name)
        (let ((default-directory (file-name-as-directory dat401-repo))
              (process-environment (dat401--env)))
          (let ((buf (dat401--term-new name)))
            (with-current-buffer buf
              (setq dat401--this-pane pane
                    dat401--fresh t)
              ;; Nothing but the terminal: no header, no mode line naming the pane.
              (setq-local header-line-format nil mode-line-format nil))
            buf)))))

;;;; Layout

(defun dat401--normalize (panes)
  (cond ((memq 'load panes) '(load))
        ((memq 'b panes) '(a b))
        ((memq 'claude panes) '(a claude))
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
      (`(a claude) (set-window-buffer (split-window-right) (dat401--pane 'claude)))
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
          (dat401--term-clear)))
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

;;;; The AI agent (step 4)

(defun dat401-agent (text)
  "Open a terminal beside pane A, start the agent (Claude Code) in it, and
type TEXT into its prompt. It's never submitted: no Enter and no newline is
ever sent (paragraphs are typed as one), and `dat401-quit-agent' closes it."
  (dat401-unhighlight)
  (setq dat401--hurry nil)
  (dat401-open 'claude)
  (dat401--type-line 'claude dat401-agent-command)
  (dat401--wait-for (lambda () (string-match-p dat401-agent-ready (dat401--pane-text 'claude))) 30)
  (setq dat401--hurry nil)
  (dat401--sleep (dat401--pause 1500 0.3))
  (let ((text (replace-regexp-in-string "[ \t]*\n[ \t\n]*" " " (string-trim text)))
        (dat401-typing-speed (* dat401-typing-speed dat401-agent-typing-speed)))
    (dolist (cd (dat401--delays text))
      (dat401--send 'claude (car cd))
      (dat401--sleep (cdr cd))))
  (setq dat401--hurry nil))

(defun dat401-quit-agent ()
  "Close the agent's frame without submitting anything: the window goes
first, then the agent, quietly."
  (when (memq 'claude dat401--layout)
    (apply #'dat401-layout (or (remq 'claude dat401--layout) '(a))))
  (when-let* ((buf (get-buffer (dat401--buffer-name 'claude))))
    (ignore-errors (dat401--kill-terminal buf))))

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

(defconst dat401--scraped
  '(("kinesis-stream-arn" . "\"StreamARN\": \"\\([^\"\n]+\\)\""))
  "Placeholders read from the latest output in the pane being typed into.")

(defun dat401--last-match (regexp text)
  "Group 1 of the last match of REGEXP in TEXT."
  (let ((pos 0) found)
    (while (string-match regexp text pos)
      (setq found (match-string 1 text) pos (match-end 0)))
    found))

(defun dat401--value (name pane)
  "Value for <NAME>, typing into PANE."
  (cond
   ((assoc name dat401--scraped)
    (or (dat401--wait-for (lambda () (dat401--last-match (cdr (assoc name dat401--scraped))
                                                         (dat401--pane-text pane)))
                          15)
        (string-trim (read-string (format "%s: " name)))))
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

(defconst dat401--mark-window 40000
  "Highlights are looked for in this many characters at the end of a pane.")

(defun dat401--unwrapped (beg end)
  "The text from BEG to END without the newlines the terminal added to wrap
long lines, and those newlines' positions (ascending)."
  (let ((wraps '()) (pos beg))
    (while (setq pos (text-property-any pos end (dat401--wrap-property) t))
      (push pos wraps)
      (setq pos (1+ pos)))
    (setq wraps (nreverse wraps))
    (let ((text (buffer-substring-no-properties beg end)))
      (dolist (w (reverse wraps))
        (setq text (concat (substring text 0 (- w beg)) (substring text (1+ (- w beg))))))
      (cons text wraps))))

(defun dat401--to-pos (beg wraps i)
  "Buffer position of index I in the unwrapped text that starts at BEG."
  (let ((pos (+ beg i)))
    (dolist (w wraps pos)
      (when (<= w pos) (setq pos (1+ pos))))))

(defun dat401--apply-marks (pane)
  "(Re)draw the highlights in PANE: every group of the last match of each
mark's regexp, matched as if the terminal hadn't wrapped long lines."
  (when-let* ((buf (get-buffer (dat401--buffer-name pane))))
    (with-current-buffer buf
      (mapc #'delete-overlay dat401--mark-overlays)
      (setq dat401--mark-overlays nil)
      (save-match-data
        (let* ((beg (max (point-min) (- (point-max) dat401--mark-window)))
               (u (dat401--unwrapped beg (point-max)))
               (text (car u)) (wraps (cdr u)))
          (dolist (m dat401--marks)
            (when (eq (car m) pane)
              (let ((pos 0) last)
                (while (string-match (cadr m) text pos)
                  (setq last (match-data) pos (max (1+ (match-beginning 0)) (match-end 0))))
                (when last
                  (set-match-data last)
                  (cl-loop for g from 1 while (match-beginning g)
                           do (let ((ov (make-overlay (dat401--to-pos beg wraps (match-beginning g))
                                                      (dat401--to-pos beg wraps (match-end g)))))
                                (overlay-put ov 'face 'dat401-mark)
                                (overlay-put ov 'priority 100)
                                (push ov dat401--mark-overlays))))))))))))

(defun dat401--after-redraw (&rest _)
  "Terminals redraw by replacing lines, which takes overlays with them."
  (when (and dat401--marks dat401--this-pane)
    (dat401--apply-marks dat401--this-pane)))

(defun dat401--after-ghostel-redraw (buffer &rest _)
  (when (and dat401--marks (buffer-live-p buffer))
    (with-current-buffer buffer (dat401--after-redraw))))

(with-eval-after-load 'vterm
  (advice-add 'vterm--redraw :after #'dat401--after-redraw))
(with-eval-after-load 'ghostel
  (advice-add 'ghostel--redraw-now :after #'dat401--after-ghostel-redraw))

(defun dat401--mark (pane regexp)
  (push (list pane regexp) dat401--marks)
  (dat401--apply-marks pane))

(defconst dat401--scan-regexp
  (concat "\\(\\(?:Index\\(?: Only\\)? \\|Seq \\)Scan\\(?: Backward\\)? using [a-z0-9_]+\\)"
          "[^\n]*(actual time=[0-9.]+ \\(rows=[0-9]+\\)")
  "A plan's scan node and the rows it actually read.")

(defconst dat401--named-marks
  `(("scan" . ,dat401--scan-regexp)
    ("index" . ,dat401--scan-regexp)
    ("sort" . "\\(Sort Key: [^\n]+\\)\n *\\(Sort Method: [^\n]*?\\)  ")
    ("filter" . "\\(Filter: [^\n]+\\)\n *\\(Rows Removed by Filter: [0-9]+\\)")
    ("cond" . "\\(Index Cond: [^\n]+\\)")
    ("time" . "\\(Execution Time: [0-9.]+ ms\\)")
    ("dpu" . "\\(Total: [0-9.]+ DPU\\)"))
  "Parts of an EXPLAIN ANALYZE plan to highlight by name. Anything else is a
JSON field.")

(defun dat401-highlight (pane &rest whats)
  "Highlight WHATS in PANE's latest output: parts of an EXPLAIN plan (scan,
sort, filter, cond, time, dpu), or the values of JSON fields (identifier)."
  (dolist (what whats)
    (dat401--mark pane (or (cdr (assoc what dat401--named-marks))
                           (format "\"%s\": \"\\([^\"\n]+\\)\"" (regexp-quote what))))))

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
      (let ((ms (if burst (dat401--jitter 45 0.25) (dat401--jitter 78 0.4))))
        (when (eq prev ?\s)
          (cl-incf ms (dat401--jitter 125 0.45))
          (when (< (random 100) 8)                       ; thinking about the next word
            (cl-incf ms (dat401--jitter 650 0.45))))
        (when (memq prev '(?, ?\; ?\( ?\) ?. ?= ?:))
          (cl-incf ms (dat401--jitter 135 0.5)))
        (when (or (memq ch dat401--shifted)              ; Shift, or a reach
                  (and (<= ?A ch ?Z) (not (and prev (<= ?A prev ?Z)))))
          (cl-incf ms (dat401--jitter 60 0.4)))
        (when (< (random 1000) 15)                       ; a hiccup
          (cl-incf ms (dat401--jitter 300 0.4)))
        (push (cons (char-to-string ch) (* (/ (* ms tempo) 1000.0) dat401-typing-speed)) out)
        (setq prev ch)))
    (nreverse out)))


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
  (with-current-buffer (dat401--pane pane) (dat401--term-send string paste)))

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
    ("cluster-endpoint" . "\"endpoint\": \"")
    ("kinesis-stream-arn" . "\"StreamARN\": \""))
  "Where on screen a value is copied from: the text just before it. A value
with no entry is copied from wherever it last appears.")

(defvar dat401--copies 0 "How many copies have been acted out (for tests).")
(defvar dat401--recalls (cons 0 0) "(FOUND . TYPED-INSTEAD) Ctrl-R re-runs (for tests).")

(defun dat401--id-like-p (value)
  "Is VALUE something nobody types by hand (a job id, a cluster id)?"
  (string-match-p "\\`[a-z0-9]\\{20,\\}\\'" value))

(defun dat401--wrap-tolerant (s)
  "A regexp for S that still matches where the terminal wrapped it."
  (mapconcat (lambda (c) (regexp-quote (char-to-string c))) s "\n?"))

(defun dat401--find-on-screen (buf value context)
  "(START . END) of the last VALUE in BUF's output above the current input
line (after CONTEXT, if given), or nil."
  (with-current-buffer buf
    (save-excursion
      (goto-char (point-max))
      (skip-chars-backward " \t\n")
      (forward-line 0)
      (when (re-search-backward (concat (regexp-quote (or context ""))
                                        "\\(" (dat401--wrap-tolerant value) "\\)")
                                nil t)
        (cons (match-beginning 1) (match-end 1))))))

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
         (found (and win (not dat401--hurry) (dat401--find-on-screen buf value context)))
         (start (car found)))
    (when found
      (cl-incf dat401--copies)
      (with-selected-window win
        (with-current-buffer buf
          (dat401--sleep (dat401--pause 400 0.3))
          (dat401--term-copy-mode t)
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
                (goto-char (cdr found))
                (dat401--sleep (dat401--pause 550 0.3))
                (deactivate-mark))
            (dat401--term-copy-mode nil))))
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
    (dat401--sleep (dat401--pause 420 0.35))
    (dolist (seg (dat401--segments line))
      (if (cdr seg)
          (let ((v (dat401--value (car seg) pane)))
            (dat401--insert-value pane (car seg) typed v)
            (setq typed (concat typed v)))
        (dolist (cd (dat401--delays (car seg)))
          (dat401--send pane (car cd))
          (dat401--sleep (cdr cd)))
        (setq typed (concat typed (car seg)))))
    (dat401--sleep (dat401--pause 300 0.4))
    (with-current-buffer (dat401--pane pane) (dat401--term-return))))

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

(defun dat401-recall (pane text search)
  "Run TEXT again in PANE from psql's history, as you would: Ctrl-R, a few
characters of it (SEARCH), a look at what it found, Enter. If the search
fails, it's abandoned (Ctrl-G) and TEXT is typed instead."
  (dat401-unhighlight)
  (dat401--start (list pane))
  (select-window (get-buffer-window (dat401--pane pane)))
  (let ((buf (dat401--pane pane)))
    (dat401--sleep (dat401--pause 600 0.3))
    (with-current-buffer buf (dat401--term-ctrl "r"))
    (dat401--sleep (dat401--pause 400 0.3))
    (dolist (cd (dat401--delays search))
      (dat401--send pane (car cd))
      (dat401--sleep (cdr cd)))
    (dat401--sleep (dat401--pause 800 0.3))
    (let ((tail (dat401--pane-text pane)))
      (if (string-match-p "failed reverse-i-search" (substring tail (max 0 (- (length tail) 400))))
          (progn
            (cl-incf (cdr dat401--recalls))
            (with-current-buffer buf (dat401--term-ctrl "g"))
            (dat401--sleep 0.3)
            (dat401--type pane text))
        (cl-incf (car dat401--recalls))
        (with-current-buffer buf (dat401--term-return))
        (dat401--wait-prompt pane))))
  (setq dat401--hurry nil))

(defun dat401-copy-into (from to text)
  "Run TEXT in pane TO the way you would right after running it in FROM: put
TO on screen, then, for each line, copy it from FROM's screen (copy mode: up
to it, select it, copy), paste it into TO, and Enter. A line that isn't on
FROM's screen is typed."
  (unless (dat401--placeholders text)
    (dat401-unhighlight))
  (setq dat401--hurry nil)
  (dat401--show to)
  (dolist (line (split-string text "\n" t))
    (let ((copied (dat401--copy-from-screen from line)))
      (select-window (get-buffer-window (dat401--pane to)))
      (if (not copied)
          (dat401--type-line to line)
        (dat401--sleep (dat401--pause 300 0.3))
        (dat401--send to line)
        (dat401--sleep (dat401--pause 450 0.35))
        (with-current-buffer (dat401--pane to) (dat401--term-return)))
      (when (dat401--statement-end-p line)
        (dat401--wait-prompt to))))
  (setq dat401--hurry nil))

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
    (with-current-buffer (dat401--pane pane) (dat401--term-return))
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
  (dat401--big-font t)
  (setq demo-it--start-single-window nil)
  (dat401-layout 'a)
  (demo-it-start nil t)
  (message "DAT401: F12 / PgDn runs the next step, C-F12 jumps, any key finishes typing"))

(defun dat401-stop ()
  "Leave the demo (panes stay)."
  (interactive)
  (dat401-stage-mode -1)
  (dat401--big-font nil)
  (demo-it-disable-mode))

(defun dat401-reset ()
  "Leave the demo, close every pane (ending their psql sessions) and forget
placeholder values like the cluster endpoint. The next `dat401-start' starts
clean."
  (interactive)
  (ignore-errors (dat401-stop))
  (dolist (name (mapcar (lambda (p) (dat401--buffer-name (car p))) dat401--pane-titles))
    (dat401--kill-terminal (get-buffer name)))
  (mapc (lambda (b) (when (buffer-live-p b) (kill-buffer b))) dat401--views)
  (clrhash dat401--vars)
  (setq dat401--views nil dat401--marks nil dat401--layout nil dat401--section ""
        dat401--stale-endpoints nil)
  (message "DAT401: reset. M-x dat401-start to begin again"))

(provide 'dat401-stage)
;;; dat401-stage.el ends here

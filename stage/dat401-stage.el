;;; dat401-stage.el --- Typed-looking terminal demos for DAT401 (re:Invent 2026)  -*- lexical-binding: t; -*-

;; The runtime for the talk's demo-it script (dat401-talk.el, generated from
;; TALK.md by generate.py). It provides:
;;
;;   - panes: vterm buffers for psql A, psql B, a shell, the events viewer and
;;     the load generator, all started in the repo root;
;;   - layouts: one pane, two side by side, or one over another;
;;   - human-paced typing: bursts within words, pauses between them and after
;;     punctuation, the odd hesitation, and a beat before Enter;
;;   - banners for the talk's "★ DSQL difference" key points, a side window for
;;     AI prompts, and a header line naming the current step;
;;   - code on screen: the real file, narrowed to an excerpt from TALK.md, with
;;     the excerpt's lines highlighted and the lines between them dimmed;
;;   - placeholders filled in as they're typed: <job_id> from the pane's own
;;     output, <cluster-endpoint> from `create-cluster's, anything else
;;     (<kinesis-stream>, ...) asked for once.
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
(defcustom dat401-text-scale 2 "Text scale for panes, banners and prompts." :type 'integer)

(defcustom dat401-typing-speed 1.0
  "Multiplier on typing delays: 1.0 is a confident typist, 0.5 twice as fast,
0 instant."
  :type 'number)

(defcustom dat401-wait-timeout 180
  "Seconds to wait for psql to come back before typing the next statement."
  :type 'integer)

(defvar dat401--vars (make-hash-table :test 'equal)
  "Placeholder values, e.g. \"cluster-endpoint\".")

(defvar dat401--section "" "The current talk step, shown in the header line.")
(defvar dat401--layout nil "The panes on screen, e.g. (a b).")
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
(defvar-local dat401--title nil "Header title, if not the pane's (e.g. a file name).")

(defun dat401--header ()
  (propertize (format " %s   %s "
                      (or dat401--title (alist-get dat401--this-pane dat401--pane-titles))
                      dat401--section)
              'face '(:weight bold :inherit header-line)))

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
                (text-scale-set dat401-text-scale)
                (setq-local header-line-format '(:eval (dat401--header))))
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
    (dat401--clear-overlays)
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
    (apply #'dat401-layout (cons pane dat401--layout))))

(defun dat401-section (title)
  "Name the current talk step in every pane's header line."
  (setq dat401--section title)
  (force-mode-line-update t))

;;;; Banners and prompts

(defconst dat401--overlay-buffers '("*DSQL difference*" "*prompt*"))

(defun dat401--clear-overlays ()
  (dolist (name dat401--overlay-buffers)
    (when-let* ((win (get-buffer-window name)))
      (when (window-deletable-p win) (delete-window win)))))

(defun dat401--fill-buffer (name text face)
  (with-current-buffer (get-buffer-create name)
    (let ((inhibit-read-only t))
      (erase-buffer)
      (insert (propertize text 'face face))
      (goto-char (point-min)))
    (text-scale-set (1+ dat401-text-scale))
    (setq-local mode-line-format nil header-line-format nil cursor-type nil
                word-wrap t truncate-lines nil)
    (visual-line-mode 1)
    (read-only-mode 1)
    (current-buffer)))

(defun dat401-key (title &optional body)
  "Show a ★ key point as a banner across the top of the frame."
  (dat401--clear-overlays)
  (let* ((buf (dat401--fill-buffer "*DSQL difference*"
                                   (concat title (if body (concat "\n" body) ""))
                                   '(:weight bold :foreground "black" :background "#ffd75f")))
         (win (split-window (frame-root-window) (- (max 5 (/ (frame-height) 3))) 'above)))
    (set-window-buffer win buf)
    (set-window-dedicated-p win t)))

(defun dat401-prompt (text &optional produced)
  "Show an AI prompt (and what it PRODUCED) in a side window."
  (dat401--clear-overlays)
  (let* ((text (dat401--resolve-static text))
         (buf (dat401--fill-buffer "*prompt*"
                                   (concat "AI prompt\n\n" text
                                           (if produced (concat "\n\nWhat it produced\n\n" produced) ""))
                                   '(:inherit default)))
         (win (split-window (frame-root-window) (- (floor (frame-width) 2)) 'right)))
    (set-window-buffer win buf)
    (set-window-dedicated-p win t)))

;;;; Code on screen

(defface dat401-code-mark
  '((((background dark)) :extend t :background "#2e3b52")
    (t :extend t :background "#fff1c2"))
  "The excerpt's lines.")

(defface dat401-code-dim '((t :inherit shadow))
  "Lines of the file between the excerpt's.")

(defun dat401--code-mode (path)
  "Set the major mode for PATH without its hooks: no LSP, no linters on stage."
  (let* ((m (assoc-default path auto-mode-alist #'string-match))
         (m (if (consp m) (car m) m))
         (modes (delq nil (list (and (functionp m) m)
                                (and (string-match-p "\\.[jt]sx?\\'" path) #'js-mode)
                                #'prog-mode))))
    (cl-loop for mode in modes
             until (ignore-errors (delay-mode-hooks (funcall mode)) t))
    (font-lock-mode 1)
    (font-lock-ensure)))

(defun dat401--line-pos (n)
  "Position of the start of line N (1-based) in the current (widened) buffer."
  (save-excursion (goto-char (point-min)) (forward-line (1- n)) (point)))

(defun dat401-code (file ranges)
  "Show FILE (relative to the repo) full screen, narrowed to RANGES' span.
RANGES are (FIRST . LAST) line numbers: the excerpt TALK.md shows. Those
lines are highlighted; the lines between them are dimmed."
  (dat401--clear-overlays)
  (let ((path (expand-file-name file dat401-repo))
        (buf (get-buffer-create "*code*")))
    (with-current-buffer buf
      (let ((inhibit-read-only t))
        (widen)
        (remove-overlays)
        (erase-buffer)
        (insert-file-contents path)
        (dat401--code-mode path)
        (cl-loop for (a . b) in ranges
                 for prev = nil then last
                 for last = b
                 do (let ((ov (make-overlay (dat401--line-pos a) (dat401--line-pos (1+ b)))))
                      (overlay-put ov 'face 'dat401-code-mark))
                 when (and prev (> a (1+ prev)))
                 do (overlay-put (make-overlay (dat401--line-pos (1+ prev)) (dat401--line-pos a))
                                 'face 'dat401-code-dim))
        (narrow-to-region (dat401--line-pos (caar ranges))
                          (dat401--line-pos (1+ (cdar (last ranges))))))
      (setq dat401--this-pane 'code
            dat401--title file)
      (setq-local display-line-numbers-widen t
                  header-line-format '(:eval (dat401--header))
                  cursor-type nil)
      (display-line-numbers-mode 1)
      (text-scale-set dat401-text-scale)
      (read-only-mode 1)
      (goto-char (point-min)))
    (delete-other-windows)
    (switch-to-buffer buf)
    (set-window-start nil (point-min))
    (setq dat401--layout '(code))))

;;;; Placeholders

(defun dat401--placeholders (text)
  "The <names> in TEXT, in order."
  (let ((pos 0) out)
    (while (string-match "<\\([A-Za-z][A-Za-z0-9_-]*\\)>" text pos)
      (push (match-string 1 text) out)
      (setq pos (match-end 0)))
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

;;;; Typing

(defun dat401--rand (lo hi) (+ lo (random (1+ (- hi lo)))))

(defun dat401--delays (line)
  "A list of (STRING . SECONDS): LINE typed by a person. Fast bursts inside
words, a pause between words, a longer one after punctuation, and now and
then a hesitation before a word."
  (let ((out '()) (prev nil) (burst (dat401--rand 3 7)))
    (dolist (ch (string-to-list line))
      (let ((ms (dat401--rand 18 45)))
        (when (eq prev ?\s)
          (cl-incf ms (dat401--rand 30 110))
          (when (< (random 100) 4) (cl-incf ms (dat401--rand 200 450))))
        (when (memq prev '(?, ?\; ?\( ?\) ?. ?=))
          (cl-incf ms (dat401--rand 40 120)))
        (cl-decf burst)
        (when (<= burst 0)
          (cl-incf ms (dat401--rand 15 80))
          (setq burst (dat401--rand 3 8)))
        (push (cons (char-to-string ch) (* (/ ms 1000.0) dat401-typing-speed)) out)
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

(defun dat401--type-line (pane line)
  (let ((buf (dat401--pane pane)))
    (dat401--sleep (* (/ (dat401--rand 200 500) 1000.0) dat401-typing-speed))
    (dolist (cd (dat401--delays line))
      (with-current-buffer buf (vterm-send-string (car cd)))
      (dat401--sleep (cdr cd)))
    (dat401--sleep (* (/ (dat401--rand 150 400) 1000.0) dat401-typing-speed))
    (with-current-buffer buf (vterm-send-return))))

(defun dat401--statement-end-p (line)
  "Does LINE finish something psql will run (a ;, or a backslash command)?"
  (let ((l (string-trim line)))
    (or (string-suffix-p ";" l) (string-prefix-p "\\" l) (string-match-p "\\\\gset\\'" l))))

(defun dat401--type (pane text)
  "Type TEXT into PANE line by line. Between lines, wait for a shell command
or a psql statement to finish, so nothing is typed over its output. Never
wait after the last line: it may start something that keeps running."
  (let ((lines (split-string text "\n" t)))
    (while lines
      (let* ((line (dat401--resolve (pop lines) pane))
             (shell (dat401--shell-prompt-p pane)))
        (when (string-match-p "aws dsql create-cluster" line)
          ;; A new cluster: its endpoint is whichever one appears next.
          (setq dat401--stale-endpoints
                (apply #'append (mapcar (lambda (p)
                                          (when (get-buffer (dat401--buffer-name p))
                                            (dat401--endpoints (dat401--pane-text p))))
                                        '(a b shell))))
          (remhash "cluster-endpoint" dat401--vars))
        (dat401--type-line pane line)
        (when (and lines (or shell (dat401--statement-end-p line)))
          (sit-for 0.3)
          (dat401--wait-for (lambda () (dat401--at-prompt-p pane)) dat401-wait-timeout))))))

(defun dat401-run (panes text)
  "Type TEXT into each of PANES (a pane symbol or a list), one after the other."
  (let ((panes (if (listp panes) panes (list panes))))
    (setq dat401--hurry nil)
    (dat401--clear-overlays)
    ;; Put every target pane on screen first, so the layout doesn't change
    ;; halfway through.
    (dolist (p panes) (dat401--show p))
    (dolist (p panes)
      (dat401--show p)
      (select-window (get-buffer-window (dat401--pane p)))
      (dat401--type p text)
      (setq dat401--hurry nil))))

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
  (dolist (name (append (mapcar (lambda (p) (dat401--buffer-name (car p))) dat401--pane-titles)
                        '("*code*") dat401--overlay-buffers))
    (when-let* ((buf (get-buffer name)))
      (when-let* ((proc (get-buffer-process buf)))
        (set-process-query-on-exit-flag proc nil))
      (kill-buffer buf)))
  (clrhash dat401--vars)
  (setq dat401--layout nil dat401--section "" dat401--stale-endpoints nil)
  (message "DAT401: reset. M-x dat401-start to begin again"))

(provide 'dat401-stage)
;;; dat401-stage.el ends here

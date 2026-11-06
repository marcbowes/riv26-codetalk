;;; dat401-test.el --- ERT tests for dat401-stage's pure parts  -*- lexical-binding: t; -*-
;;
;; emacs -Q --batch -L <demo-it> -L <vterm> -L stage -l stage/test/dat401-test.el -f ert-run-tests-batch-and-exit

(require 'ert)
(require 'dat401-stage)

;; These test the stage's logic on plain buffers, with the terminal's
;; functions stubbed (vterm's names). The real terminal (ghostel by default)
;; is exercised end to end by e2e.py.
(setq dat401-terminal 'vterm)

(ert-deftest dat401-delays-type-every-char ()
  (let* ((line "SELECT title, created_at FROM posts ORDER BY created_at DESC LIMIT 20;")
         (ds (dat401--delays line)))
    (should (equal (mapconcat #'car ds "") line))
    (should (cl-every (lambda (d) (> (cdr d) 0)) ds))))

(defun dat401-test--stats (xs)
  (let* ((n (float (length xs)))
         (mean (/ (apply #'+ xs) n))
         (sd (sqrt (/ (apply #'+ (mapcar (lambda (x) (expt (- x mean) 2)) xs)) n))))
    (cons mean sd)))

(ert-deftest dat401-delays-are-human-ish ()
  ;; About 7-12 characters a second over a typical line: a quick person, not
  ;; a machine's 20+.
  (let* ((line "SELECT title, created_at FROM posts ORDER BY created_at DESC LIMIT 20;")
         (all (cl-loop repeat 50 append (mapcar #'cdr (dat401--delays line))))
         (stats (dat401-test--stats all)))
    (should (< 0.07 (car stats) 0.16))
    ;; Uneven: the spread between keys is as big as the average key.
    (should (> (/ (cdr stats) (car stats)) 0.7)))
  ;; Some lines are quicker than others.
  (let ((totals (cl-loop repeat 40 collect
                         (apply #'+ (mapcar #'cdr (dat401--delays "CREATE TABLE posts ("))))))
    (should (> (- (apply #'max totals) (apply #'min totals)) 0.8)))
  ;; A gap after a space, on average.
  (let ((after-space (cl-loop repeat 200 collect (cdr (nth 2 (dat401--delays "a b")))))
        (in-word (cl-loop repeat 200 collect (cdr (nth 1 (dat401--delays "ab"))))))
    (should (> (car (dat401-test--stats after-space)) (* 1.8 (car (dat401-test--stats in-word)))))))

(ert-deftest dat401-delays-scale-with-speed ()
  (let ((dat401-typing-speed 0))
    (should (cl-every (lambda (d) (= (cdr d) 0)) (dat401--delays "hello world")))))

(ert-deftest dat401-placeholders ()
  (should (equal (dat401--placeholders "CALL sys.wait_for_job('<job_id>');") '("job_id")))
  (should (equal (dat401--placeholders "export PGHOST=<cluster-A-id>.dsql x <kinesis-stream>")
                 '("cluster-A-id" "kinesis-stream")))
  (should (null (dat401--placeholders "SELECT 1 WHERE a < b AND c > d"))))

(ert-deftest dat401-last-job-id ()
  (let ((out (concat "postgres=> CREATE INDEX ASYNC a ON t (x);\n"
                     "           job_id           \n----------------------------\n"
                     " bk7zsxu5xjh6pgsaskjqxpb5ty\n(1 row)\n\n"
                     "postgres=> SELECT job_id, status FROM sys.jobs;\n"
                     "           job_id           |  status\n----------------------------+-----------\n"
                     " zzzzzzzzzzzzzzzzzzzzzzzzzz | submitted\n(1 row)\n\n"
                     "postgres=> ALTER TABLE ASYNC c VALIDATE CONSTRAINT k;\n"
                     "           job_id           \n----------------------------\n"
                     " 4ygtvgcecba5hdzswg3cc4pdqy\n(1 row)\n\npostgres=> ")))
    ;; The newest single-column job_id result, not a sys.jobs row.
    (should (equal (dat401--last-job-id out) "4ygtvgcecba5hdzswg3cc4pdqy"))
    (should (equal (dat401--last-job-id (substring out 0 (string-match "postgres=> SELECT" out)))
                   "bk7zsxu5xjh6pgsaskjqxpb5ty"))
    (should (null (dat401--last-job-id "postgres=> SELECT 1;")))))

(ert-deftest dat401-statement-end ()
  (should (dat401--statement-end-p "SELECT 1;"))
  (should (dat401--statement-end-p "\\c"))
  (should (dat401--statement-end-p "SELECT id AS post_id FROM posts LIMIT 1 \\gset"))
  (should-not (dat401--statement-end-p "CREATE TABLE posts ("))
  (should-not (dat401--statement-end-p "export PGHOST=x")))

(ert-deftest dat401-layouts ()
  (should (equal (dat401--normalize '(b a)) '(a b)))
  (should (equal (dat401--normalize '(shell)) '(a shell)))
  (should (equal (dat401--normalize '(events a)) '(a events)))
  (should (equal (dat401--normalize '(load a)) '(load)))
  (should (equal (dat401--normalize '(a)) '(a))))

(ert-deftest dat401-generated-steps-load ()
  (load (expand-file-name "stage/dat401-talk.el" dat401-repo) nil t)
  (should (> (length demo-it--steps) 40))
  (should (assoc "Step 7 · A race, and foreign keys" dat401--steps-index))
  ;; Every step is a form demo-it can eval.
  (should (cl-every #'consp demo-it--steps)))

(ert-deftest dat401-code-shows-the-excerpt ()
  (let* ((dir (make-temp-file "dat401" t))
         (dat401-repo dir))
    (with-temp-file (expand-file-name "f.ts" dir)
      (insert "// header\nimport x;\nimport y;\n\nfunction f() {\n  body;\n  return 1;\n}\n// tail\n"))
    (dat401-code "f.ts" '((2 . 3) (7 . 8)))
    ;; Opened like a file: named after it, no header line.
    (should (equal (buffer-name (window-buffer)) "f.ts"))
    (with-current-buffer "f.ts"
      (should (null header-line-format))
      ;; Narrowed to the span: lines 2-8.
      (should (equal (buffer-string)
                     "import x;\nimport y;\n\nfunction f() {\n  body;\n  return 1;\n}\n"))
      (should buffer-read-only)
      (cl-flet ((faces-at (s) (save-excursion
                                (goto-char (point-min)) (search-forward s)
                                (mapcar (lambda (o) (overlay-get o 'face)) (overlays-at (1- (point)))))))
        (should (memq 'dat401-code-mark (faces-at "import y")))
        (should (memq 'dat401-code-mark (faces-at "return 1")))
        (should (memq 'dat401-code-dim (faces-at "body")))
        (should-not (memq 'dat401-code-dim (faces-at "import x")))))
    (should (equal dat401--layout '(view)))
    ;; Showing another excerpt replaces the first.
    (dat401-code "f.ts" '((5 . 5)))
    (should (= 1 (cl-count-if (lambda (b) (string-prefix-p "f.ts" (buffer-name b))) (buffer-list))))
    (with-current-buffer "f.ts"
      (should (equal (buffer-string) "function f() {\n"))
      (should (= 1 (length (overlays-in (point-min) (point-max))))))
    (delete-directory dir t)))

(ert-deftest dat401-endpoint-from-create-cluster ()
  (let ((out (concat "❯ aws dsql create-cluster --region us-west-2\n{\n"
                     "    \"identifier\": \"e5ugy6wru5xgmpi4lx4e6sxs54\",\n"
                     "    \"status\": \"CREATING\",\n"
                     "    \"endpoint\": \"e5ugy6wru5xgmpi4lx4e6sxs54.dsql.us-west-2.on.aws\"\n}\n")))
    (should (equal (dat401--endpoints out) '("e5ugy6wru5xgmpi4lx4e6sxs54.dsql.us-west-2.on.aws")))
    (should (equal (dat401--endpoints (concat out (replace-regexp-in-string "e5ug" "zzzz" out)))
                   '("e5ugy6wru5xgmpi4lx4e6sxs54.dsql.us-west-2.on.aws"
                     "zzzzy6wru5xgmpi4lx4e6sxs54.dsql.us-west-2.on.aws")))
    (should (null (dat401--endpoints "export PGHOST=x.dsql.us-west-2.on.aws")))))

(ert-deftest dat401-new-endpoint-skips-stale-ones ()
  ;; A rehearsal re-runs create-cluster in the same pane: the old endpoint is
  ;; still on screen, and must not be used for the new cluster.
  (let* ((old "    \"endpoint\": \"aaaaaaaaaaaaaaaaaaaaaaaaaa.dsql.us-west-2.on.aws\"\n")
         (new "    \"endpoint\": \"bbbbbbbbbbbbbbbbbbbbbbbbbb.dsql.us-west-2.on.aws\"\n")
         (screen old))
    (cl-letf (((symbol-function 'get-buffer) (lambda (name) (equal name "*psql A*")))
              ((symbol-function 'dat401--pane-text) (lambda (_p) screen)))
      (let ((dat401--stale-endpoints (dat401--endpoints old)))
        (should (null (dat401--new-endpoint)))
        (setq screen (concat old new))
        (should (equal (dat401--new-endpoint) "bbbbbbbbbbbbbbbbbbbbbbbbbb.dsql.us-west-2.on.aws"))))))

(ert-deftest dat401-cluster-id-from-endpoint ()
  (let ((dat401--vars (make-hash-table :test 'equal)))
    (puthash "cluster-endpoint" "e5ugy6wru5xgmpi4lx4e6sxs54.dsql.us-west-2.on.aws" dat401--vars)
    (should (equal (dat401--value "cluster-id" 'a) "e5ugy6wru5xgmpi4lx4e6sxs54"))
    (should (equal (dat401--resolve-static "The cluster endpoint is <cluster-endpoint>; id <cluster-id>.")
                   "The cluster endpoint is e5ugy6wru5xgmpi4lx4e6sxs54.dsql.us-west-2.on.aws; id e5ugy6wru5xgmpi4lx4e6sxs54."))))

(ert-deftest dat401-segments-type-text-and-paste-placeholders ()
  (should (equal (dat401--segments "export PGHOST=<cluster-id>.dsql.us-west-2.on.aws PGUSER=admin")
                 '(("export PGHOST=") ("cluster-id" . t) (".dsql.us-west-2.on.aws PGUSER=admin"))))
  (should (equal (dat401--segments "CALL sys.wait_for_job('<job_id>');")
                 '(("CALL sys.wait_for_job('") ("job_id" . t) ("');"))))
  (should (equal (dat401--segments "SELECT 1 WHERE a < b") '(("SELECT 1 WHERE a < b")))))

(ert-deftest dat401-highlights-survive-a-redraw ()
  ;; vterm redraws by deleting and re-inserting lines. Highlights come back.
  (let ((dat401--marks nil)
        (buf (get-buffer-create "*psql A*")))
    (unwind-protect
        (let ((out (concat "{\n    \"identifier\": \"e5ugy6wru5xgmpi4lx4e6sxs54\",\n"
                           "    \"arn\": \"arn:aws:dsql:us-west-2:1:cluster/e5ugy6wru5xgmpi4lx4e6sxs54\",\n"
                           "    \"endpoint\": \"e5ugy6wru5xgmpi4lx4e6sxs54.dsql.us-west-2.on.aws\"\n}\n"
                           "❯ export PGHOST=e5ugy6wru5xgmpi4lx4e6sxs54.dsql.us-west-2.on.aws\n")))
          (with-current-buffer buf (insert out) (setq dat401--this-pane 'a))
          (cl-flet ((marked () (with-current-buffer buf
                                 (sort (mapcar (lambda (o) (buffer-substring (overlay-start o) (overlay-end o)))
                                               (cl-remove-if-not (lambda (o) (eq (overlay-get o 'face) 'dat401-mark))
                                                                 (overlays-in (point-min) (point-max))))
                                       #'string<))))
            (dat401-highlight 'a "identifier")
            (dat401-highlight 'a "endpoint")
            (dat401--mark 'a "export PGHOST=\\(e5ugy6wru5xgmpi4lx4e6sxs54\\)")
            ;; Three: the identifier, the endpoint, and the pasted identifier.
            ;; Not the ARN.
            (should (equal (marked) '("e5ugy6wru5xgmpi4lx4e6sxs54" "e5ugy6wru5xgmpi4lx4e6sxs54"
                                      "e5ugy6wru5xgmpi4lx4e6sxs54.dsql.us-west-2.on.aws")))
            ;; A redraw replaces the text...
            (with-current-buffer buf (erase-buffer) (insert out) (dat401--after-redraw))
            (should (= 3 (length (marked))))
            ;; ...and clearing removes them for good.
            (dat401-unhighlight)
            (should (null (marked)))
            (with-current-buffer buf (dat401--after-redraw))
            (should (null (marked)))))
      (kill-buffer buf))))

(ert-deftest dat401-agent-prompt-is-never-submitted ()
  ;; Typed into the agent's frame with no Enter and no newline anywhere.
  (let ((sent '()) (returns 0))
    (cl-letf (((symbol-function 'dat401-open) #'ignore)
              ((symbol-function 'dat401-unhighlight) #'ignore)
              ((symbol-function 'dat401--pane-text) (lambda (_) "⏵⏵ auto mode on (shift+tab to cycle)"))
              ((symbol-function 'dat401--sleep) #'ignore)
              ((symbol-function 'dat401--send) (lambda (_p s &rest _) (push s sent)))
              ((symbol-function 'dat401--pane) (lambda (_) (current-buffer)))
              ((symbol-function 'vterm-send-return) (lambda () (cl-incf returns))))
      (dat401-agent "In ./blog, build an API.\n\nOperations: ListPosts.\nStart the server."))
    (let ((typed (apply #'concat (nreverse sent))))
      ;; The only Enter is the one that starts the agent.
      (should (= returns 1))
      (should (string-prefix-p "claude" typed))
      (should (string-suffix-p "In ./blog, build an API. Operations: ListPosts. Start the server." typed))
      (should-not (string-match-p "[\n\r]" typed)))))

(ert-deftest dat401-highlight-dpu-total ()
  (let ((dat401--marks nil)
        (buf (get-buffer-create "*psql A*")))
    (unwind-protect
        (progn
          (with-current-buffer buf
            (insert " Statement DPU Estimate:\n   Compute: 0.2 DPU\n   Total: 120.76381 DPU\n"
                    "postgres=> \n ...\n   Total: 0.04708 DPU\npostgres=> ")
            (setq dat401--this-pane 'a))
          (dat401-highlight 'a "dpu")
          (with-current-buffer buf
            ;; The latest EXPLAIN's total.
            (should (equal (mapcar (lambda (o) (buffer-substring (overlay-start o) (overlay-end o)))
                                   dat401--mark-overlays)
                           '("Total: 0.04708 DPU"))))
          ;; The next command clears it (no pane needed: nothing typed in batch).
          (cl-letf (((symbol-function 'dat401--start) #'ignore)
                    ((symbol-function 'dat401--type) #'ignore)
                    ((symbol-function 'select-window) #'ignore)
                    ((symbol-function 'get-buffer-window) #'ignore))
            (dat401-run 'a "select 1;"))
          (should (null dat401--marks))
          (with-current-buffer buf (should (null dat401--mark-overlays))))
      (kill-buffer buf))))

(ert-deftest dat401-copy-from-screen-acts-it-out ()
  ;; Up to the id's line, along it word by word, select exactly the id, then
  ;; back. vterm's copy mode is stubbed: there's no terminal in batch.
  (let* ((buf (get-buffer-create "*psql A*"))
         (id "bk7zsxu5xjh6pgsaskjqxpb5ty")
         (seen '()) (dat401--hurry nil) (dat401--copies 0) (copy-mode '())
         (transient-mark-mode t))                                 ; batch Emacs has it off
    (unwind-protect
        (progn
          (switch-to-buffer buf)
          (insert "postgres=> create index async x on t (y);\n"
                  "           job_id           \n----------------------------\n"
                  " " id "\n(1 row)\n\npostgres=> call sys.wait_for_job('")
          (cl-letf (((symbol-function 'vterm-copy-mode) (lambda (on) (push on copy-mode)))
                    ((symbol-function 'dat401--sleep)
                     (lambda (_) (push (list (point) (and (region-active-p)
                                                          (buffer-substring (region-beginning) (region-end))))
                                       seen))))
            (should (dat401--copy-from-screen 'a id))
            (should (null (dat401--copy-from-screen 'a "notonscreenatallxxxxxxxxx"))))
          (setq seen (nreverse seen))
          (should (equal (reverse copy-mode) '(1 -1)))            ; in and out of copy mode
          (should (= dat401--copies 1))
          ;; It passed through the id's line before selecting.
          (let ((id-start (save-excursion (goto-char (point-min)) (search-forward id) (match-beginning 0))))
            (should (cl-some (lambda (s) (= (car s) id-start)) seen)))
          ;; At one point exactly the id was selected; at the end, nothing is.
          (should (cl-some (lambda (s) (equal (cadr s) id)) seen))
          (should-not (region-active-p)))
      (kill-buffer buf))))

(ert-deftest dat401-copy-context-picks-the-identifier-line ()
  (let ((buf (get-buffer-create "*psql A*")))
    (unwind-protect
        (with-current-buffer buf
          (insert "{\n    \"identifier\": \"e5ugy6wru5xgmpi4lx4e6sxs54\",\n"
                  "    \"arn\": \"arn:aws:dsql:us-west-2:1:cluster/e5ugy6wru5xgmpi4lx4e6sxs54\",\n"
                  "    \"endpoint\": \"e5ugy6wru5xgmpi4lx4e6sxs54.dsql.us-west-2.on.aws\"\n}\n❯ export PGHOST=")
          (let ((pos (dat401--find-on-screen buf "e5ugy6wru5xgmpi4lx4e6sxs54" "\"identifier\": \"")))
            (goto-char (car pos))
            (should (looking-back "\"identifier\": \"" (line-beginning-position)))))
      (kill-buffer buf))))

(ert-deftest dat401-ids-are-never-typed ()
  (should (dat401--id-like-p "bk7zsxu5xjh6pgsaskjqxpb5ty"))
  (should-not (dat401--id-like-p "dat401-cdc")))

(ert-deftest dat401-copy-into-copies-each-line-from-the-other-pane ()
  (let* ((a (get-buffer-create "*psql A*"))
         (b (get-buffer-create "*psql B*"))
         (stmts '("select id as post_id from posts where slug = 'hello-dsql' \\gset"
                  "select id as user_id from users where username = 'raluca' \\gset"))
         (sent '()) (dat401--copies 0) (dat401--marks nil) (transient-mark-mode t))
    (unwind-protect
        (progn
          (with-current-buffer a
            ;; A is half width now: the first statement wrapped.
            (insert "postgres=> select id as post_id from posts where slug\n = 'hello-dsql' \\gset\n"
                    "postgres=> " (cadr stmts) "\npostgres=> "))
          (switch-to-buffer a)
          (cl-letf (((symbol-function 'dat401--show) #'ignore)
                    ((symbol-function 'get-buffer-window) (lambda (&rest _) (selected-window)))
                    ((symbol-function 'select-window) #'ignore)
                    ((symbol-function 'vterm-copy-mode) #'ignore)
                    ((symbol-function 'dat401--sleep) #'ignore)
                    ((symbol-function 'dat401--wait-prompt) #'ignore)
                    ((symbol-function 'dat401--send) (lambda (p s &rest _) (push (list p s) sent)))
                    ((symbol-function 'vterm-send-return) (lambda () (push 'ret sent))))
            (dat401-copy-into 'a 'b (mapconcat #'identity stmts "\n")))
          ;; Both copied off A's screen (the wrapped one too) and pasted into B.
          (should (= dat401--copies 2))
          (should (equal (nreverse sent)
                         (list (list 'b (car stmts)) 'ret (list 'b (cadr stmts)) 'ret))))
      (kill-buffer a) (kill-buffer b))))

(defconst dat401-test--plan
  (concat
   "postgres=> explain analyze verbose\n"
   " Limit  (cost=22273.48..22273.53 rows=20 width=51) (actual time=231.026..231.031 rows=20 loops=1)\n"
   "   ->  Sort  (cost=22273.48..22274.73 rows=500 width=51) (actual time=231.025..231.028 rows=20 loops=1)\n"
   "         Sort Key: posts.created_at DESC\n"
   "         Sort Method: top-N heapsort  Memory: 28kB\n"
   "         ->  Index Only Scan using posts_pkey on public.posts  (cost=12600.17..22260.18 rows=500 "
   "WRAP" "width=51) (actual time=0.999..228.691 rows=12441 loops=1)\n"
   "               Filter: ((posts.metadata ->> 'category'::text) = 'databases'::text)\n"
   "               Rows Removed by Filter: 87560\n"
   " Execution Time: 231.112 ms\n"
   " Statement DPU Estimate:\n   Total: 120.79467 DPU\n(21 rows)\n\npostgres=> ")
  "A real plan (step 8, before the index). WRAP marks where the terminal wrapped.")

(defun dat401-test--marked-in (buf)
  (with-current-buffer buf
    (sort (mapcar (lambda (o) (replace-regexp-in-string
                               "\n" "" (buffer-substring-no-properties (overlay-start o) (overlay-end o))))
                  dat401--mark-overlays)
          #'string<)))

(ert-deftest dat401-highlight-plan-parts ()
  (let ((dat401--marks nil)
        (buf (get-buffer-create "*psql A*")))
    (unwind-protect
        (progn
          (with-current-buffer buf
            (let ((parts (split-string dat401-test--plan "WRAP")))
              (insert (car parts))
              ;; The terminal wrapped the scan line: a newline vterm marks as a wrap.
              (insert (propertize "\n" 'vterm-line-wrap t))
              (insert (cadr parts)))
            (setq dat401--this-pane 'a))
          ;; The scan node and its actual rows, across the wrap.
          (dat401-highlight 'a "scan")
          (should (equal (dat401-test--marked-in buf) '("Index Only Scan using posts_pkey" "rows=12441")))
          (dat401-unhighlight)
          (dat401-highlight 'a "filter")
          (should (equal (dat401-test--marked-in buf)
                         '("Filter: ((posts.metadata ->> 'category'::text) = 'databases'::text)"
                           "Rows Removed by Filter: 87560")))
          (dat401-unhighlight)
          (dat401-highlight 'a "sort")
          (should (equal (dat401-test--marked-in buf)
                         '("Sort Key: posts.created_at DESC" "Sort Method: top-N heapsort")))
          (dat401-unhighlight)
          ;; Several at once, as `highlight time dpu' does.
          (dat401-highlight 'a "time" "dpu")
          (should (equal (dat401-test--marked-in buf)
                         '("Execution Time: 231.112 ms" "Total: 120.79467 DPU"))))
      (kill-buffer buf))))

(ert-deftest dat401-recall-uses-ctrl-r ()
  (let ((keys '()) (screen "postgres=> ") (typed nil))
    (cl-letf (((symbol-function 'dat401-unhighlight) #'ignore)
              ((symbol-function 'dat401--start) #'ignore)
              ((symbol-function 'select-window) #'ignore)
              ((symbol-function 'get-buffer-window) #'ignore)
              ((symbol-function 'dat401--pane) (lambda (_) (current-buffer)))
              ((symbol-function 'dat401--sleep) #'ignore)
              ((symbol-function 'dat401--wait-prompt) #'ignore)
              ((symbol-function 'dat401--pane-text) (lambda (_) screen))
              ((symbol-function 'vterm-send-key) (lambda (k &rest mods) (push (cons k (nth 2 mods)) keys)))
              ((symbol-function 'dat401--send) (lambda (_p s &rest _) (push s keys)))
              ((symbol-function 'vterm-send-return) (lambda () (push 'ret keys)))
              ((symbol-function 'dat401--type) (lambda (_p text) (setq typed text))))
      (dat401-recall 'a "explain analyze verbose\nselect 1 order by x desc limit 20;" "desc limit")
      (setq keys (nreverse keys))
      (should (equal (car keys) '("r" . t)))                       ; Ctrl-R
      (should (equal (apply #'concat (cl-remove-if-not #'stringp keys)) "desc limit"))
      (should (eq (car (last keys)) 'ret))
      (should (null typed))
      ;; If the search fails: Ctrl-G, then type it.
      (setq keys '() screen "(failed reverse-i-search)`desc limit': ")
      (dat401-recall 'a "explain analyze verbose\nselect 1;" "desc limit")
      (should (member '("g" . t) keys))
      (should-not (memq 'ret keys))
      (should (equal typed "explain analyze verbose\nselect 1;")))))

(ert-deftest dat401-e2e-script-reads ()
  ;; A broken bracket in e2e.el fails silently in a terminal Emacs (it just
  ;; sits on the dashboard), so make sure every form in it reads.
  (with-temp-buffer
    (insert-file-contents (expand-file-name "stage/test/e2e.el" dat401-repo))
    (goto-char (point-min))
    (let ((n 0))
      (condition-case nil
          (while t (read (current-buffer)) (cl-incf n))
        (end-of-file nil))
      (should (> n 4)))))

(ert-deftest dat401-kill-terminal-never-asks ()
  ;; A live process and a buffer-local kill check (as Ghostel adds) must not
  ;; turn into a question: in the demo it aborts everything.
  (let ((buf (generate-new-buffer "*claude*")))
    (with-current-buffer buf
      (add-hook 'kill-buffer-query-functions (lambda () (error "Asked to confirm")) nil t))
    (make-process :name "dat401-test-sleep" :buffer buf :command '("sleep" "30"))
    (cl-letf (((symbol-function 'yes-or-no-p) (lambda (&rest _) (error "Asked yes-or-no")))
              ((symbol-function 'y-or-n-p) (lambda (&rest _) (error "Asked y-or-n"))))
      (dat401--kill-terminal buf))
    (should-not (buffer-live-p buf))))

(ert-deftest dat401-wait-does-not-spin-on-a-pending-key ()
  ;; An F12 pressed while a step waits for psql: the wait must keep going
  ;; (and finish when the condition holds), swallow the key, and not leave
  ;; it to run the next step.
  (let ((done nil) (dat401--hurry nil))
    (run-at-time 0.3 nil (lambda () (setq done t)))
    (setq unread-command-events (list 'f12))
    (let ((t0 (float-time)))
      (should (dat401--wait-for (lambda () done) 5))
      (should (< (- (float-time) t0) 2)))
    (should (null unread-command-events))
    (should dat401--hurry)))

(ert-deftest dat401-sleep-swallows-keys-even-when-hurried ()
  (let ((dat401--hurry t))
    (setq unread-command-events (list 'f12 'f12))
    (dat401--sleep 0.05)
    (should (null unread-command-events))))

(defvar dat401-test--log nil "Steps run, for the fast-forward test (evaluated forms need a global).")

(ert-deftest dat401-fast-forward-runs-the-steps-without-the-show ()
  (let* ((demo-it--steps
          (list '(push 'intro dat401-test--log)
                '(push 'create dat401-test--log)
                '(progn (dat401-highlight 'a "dpu") (push 'highlighted dat401-test--log))
                '(progn (dat401-code "nope.ts" '((1 . 1))) (push 'code dat401-test--log))
                '(push 'schema dat401-test--log)
                '(error "boom")
                '(push 'never dat401-test--log)))
         (demo-it--step 1)
         (dat401--marks nil))
    (setq dat401-test--log nil)
    (cl-letf (((symbol-function 'dat401--wait-for) (lambda (&rest _) t))
              ((symbol-function 'dat401--wait-settled) (lambda (&rest _) t)))
      ;; Steps 2-5 run, in order; the highlight and the code view do nothing.
      (should (dat401--fast-forward 2 5))
      (should (equal (reverse dat401-test--log) '(create highlighted code schema)))
      (should (null dat401--marks))
      (should (= demo-it--step 5))
      ;; An error stops it there: the next F12 retries the failing step.
      (setq dat401-test--log nil)
      (should-not (dat401--fast-forward 6 7))
      (should (null dat401-test--log))
      (should (= demo-it--step 5)))))

(ert-deftest dat401-wait-settled-waits-past-a-stale-prompt ()
  ;; Right after Enter the screen still shows the old prompt; then the
  ;; command's output arrives, then a new prompt. Settled means the latter.
  (let* ((screen "postgres=> ")
         (t0 (float-time)))
    (run-at-time 0.1 nil (lambda () (setq screen "postgres=> call sys.wait_for_job('x');\n")))
    (run-at-time 0.8 nil (lambda () (setq screen (concat screen " succeeded\n t\n(1 row)\n\npostgres=> "))))
    (cl-letf (((symbol-function 'dat401--pane-text) (lambda (_) screen)))
      (should (dat401--wait-settled 'a 5)))
    (should (string-suffix-p "(1 row)\n\npostgres=> " screen))
    (should (> (- (float-time) t0) 0.8))))

(ert-deftest dat401-highlights-stay-only-for-a-highlighted-paste ()
  (let ((dat401--marks nil)
        (dat401--vars (make-hash-table :test 'equal))
        (dat401--stale-endpoints nil)
        (buf (get-buffer-create "*psql A*")))
    (unwind-protect
        (progn
          (with-current-buffer buf
            (insert "{\n    \"identifier\": \"e5ugy6wru5xgmpi4lx4e6sxs54\",\n"
                    "    \"endpoint\": \"e5ugy6wru5xgmpi4lx4e6sxs54.dsql.us-west-2.on.aws\"\n}\n"
                    "           job_id           \n----------------------------\n"
                    " acfo4u5vnngqxaahnyichcok54\n(1 row)\n\n"
                    " Execution Time: 231.078 ms\n   Total: 120.77194 DPU\n❯ ")
            (setq dat401--this-pane 'a))
          ;; Step 1: the endpoint is highlighted, and PGHOST pastes it: keep.
          (dat401-highlight 'a "identifier" "endpoint")
          (should (dat401--keeps-highlights-p "export PGHOST=<cluster-endpoint> PGUSER=admin" 'a))
          ;; Step 8: time and DPU highlighted; wait_for_job pastes a job id
          ;; that isn't highlighted: clear.
          (dat401-unhighlight)
          (dat401-highlight 'a "time" "dpu")
          (should-not (dat401--keeps-highlights-p "call sys.wait_for_job('<job_id>');" 'a))
          ;; Nothing pasted at all: clear.
          (should-not (dat401--keeps-highlights-p "select 1;" 'a)))
      (kill-buffer buf))))

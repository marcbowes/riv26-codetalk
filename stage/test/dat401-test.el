;;; dat401-test.el --- ERT tests for dat401-stage's pure parts  -*- lexical-binding: t; -*-
;;
;; emacs -Q --batch -L <demo-it> -L <vterm> -L stage -l stage/test/dat401-test.el -f ert-run-tests-batch-and-exit

(require 'ert)
(require 'dat401-stage)

(ert-deftest dat401-delays-type-every-char ()
  (let* ((line "SELECT title, created_at FROM posts ORDER BY created_at DESC LIMIT 20;")
         (ds (dat401--delays line)))
    (should (equal (mapconcat #'car ds "") line))
    (should (cl-every (lambda (d) (> (cdr d) 0)) ds))))

(ert-deftest dat401-delays-are-human-ish ()
  ;; Average about 20 characters a second, with longer pauses after spaces.
  (let* ((line (make-string 400 ?x))
         (avg (/ (apply #'+ (mapcar #'cdr (dat401--delays line))) 400.0)))
    (should (< 0.02 avg 0.08)))
  (let* ((ds (dat401--delays "a b"))
         (after-space (cdr (nth 2 ds))))
    (should (>= after-space 0.048))))

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

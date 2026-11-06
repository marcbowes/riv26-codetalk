;;; e2e.el --- Run generated steps against a real cluster, then dump the panes  -*- lexical-binding: t; -*-
;;
;; Used by stage/test/e2e.py: DAT401_E2E_CLUSTER=<id> DAT401_E2E_STEPS=1-27
;; DAT401_E2E_OUT=<dir> emacs -nw --eval '(load ".../e2e.el")'

(require 'dat401-stage)

(defun dat401-e2e--wait-text (pane regexp timeout)
  (dat401--wait-for (lambda () (string-match-p regexp (dat401--pane-text pane))) timeout))

(defun dat401-e2e ()
  (let* ((range (split-string (or (getenv "DAT401_E2E_STEPS") "1-27") "-"))
         (from (string-to-number (car range)))
         (to (string-to-number (cadr range)))
         (out (getenv "DAT401_E2E_OUT")))
    (setq dat401-typing-speed 0.05)
    (puthash "cluster-A-id" (getenv "DAT401_E2E_CLUSTER") dat401--vars)
    (load (expand-file-name "stage/dat401-talk.el" dat401-repo) nil t)
    (dat401-stage-mode 1)
    (dat401-layout 'a)
    (cl-loop for n from from to to do
             (let ((step (nth (1- n) demo-it--steps)))
               (with-temp-file (expand-file-name "progress.txt" out)
                 (insert (format "%d %S\n" n step)))
               (demo-it--execute-step step)
               ;; What a presenter does by eye: wait for the command to finish.
               (cond
                ((string-match-p "05-load-sample-data" (format "%S" step))
                 (dat401-e2e--wait-text 'shell "Done in" 180))
                ((get-buffer (dat401--buffer-name 'a))
                 (dat401--wait-for (lambda () (dat401--at-prompt-p 'a)) 60)))
               (sit-for 0.5)))
    (dolist (p '(a b shell))
      (when (get-buffer (dat401--buffer-name p))
        (with-temp-file (expand-file-name (format "pane-%s.txt" p) out)
          (insert (dat401--pane-text p)))))
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

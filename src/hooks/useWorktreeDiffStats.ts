import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef } from "react";
import type { Project, Task } from "../types";

interface Args {
  projects: Project[];
  tasks: Task[];
  setTasks: React.Dispatch<React.SetStateAction<Task[]>>;
  persistTasks: (projectId: string, allTasks: Task[]) => void;
}

/**
 * When a worktree task transitions to `done`, fetch the +/− line counts relative to base and write them back to the Task.
 * Triggered only for worktree tasks that aren't discarded and haven't been computed yet, deduped via pendingRef to avoid event re-emits or StrictMode double-calls.
 */
export function useWorktreeDiffStats({ projects, tasks, setTasks, persistTasks }: Args) {
  // The task-status event listener is attached once on mount, and the closure captures the projects/tasks from the first render.
  // A ref lets the event path read the latest values (diff-stats needs projectPath lookups and the task's current worktree fields).
  const projectsRef = useRef(projects);
  const tasksRef = useRef(tasks);
  useEffect(() => {
    projectsRef.current = projects;
  }, [projects]);
  useEffect(() => {
    tasksRef.current = tasks;
  }, [tasks]);

  const pendingRef = useRef<Set<string>>(new Set());

  const scheduleForDoneTask = useCallback(
    (taskId: string) => {
      const task = tasksRef.current.find((t) => t.id === taskId);
      if (!task) return;
      if (!task.worktreePath || !task.baseBranch) return;
      if (task.worktreeDiscarded) return;
      // Only counts as "computed" once both are written, to avoid legacy half-broken data (only additions or only deletions) forever showing 0
      if (task.additions !== undefined && task.deletions !== undefined) return;
      if (pendingRef.current.has(task.id)) return;
      const project = projectsRef.current.find((p) => p.id === task.projectId);
      if (!project) return;

      pendingRef.current.add(task.id);
      invoke<{ additions: number; deletions: number }>("worktree_diff_stats", {
        projectPath: project.path,
        worktreePath: task.worktreePath,
        baseBranch: task.baseBranch,
      })
        .then(({ additions, deletions }) => {
          setTasks((prev) => {
            let changed = false;
            const next = prev.map((t) => {
              if (t.id !== task.id) return t;
              if (t.additions === additions && t.deletions === deletions) return t;
              changed = true;
              return { ...t, additions, deletions };
            });
            if (changed) persistTasks(task.projectId, next);
            return changed ? next : prev;
          });
        })
        .catch((e: unknown) => {
          // Status is already done, so don't disturb the user; but log for diagnosing merge-base / path / git errors
          console.warn(`[worktree-diff-stats] task ${task.id} failed:`, e);
        })
        .finally(() => {
          pendingRef.current.delete(task.id);
        });
    },
    [setTasks, persistTasks],
  );

  return { scheduleForDoneTask };
}

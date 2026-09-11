package ai.cua.driver

internal class Refusal(val reason: String, val code: Int = 3) : Exception(reason)

internal data class TaskPlacement(val id: Int, val display: Int, val basePackage: String?, val topPackage: String?)

/** Only tasks created during this session may be brought to the front. */
internal class SessionTasks(private val allowed: Set<String>, private val display: Int) {
    private val owned = linkedMapOf<String, Int>()
    val ids: Set<Int> get() = owned.values.toSet()
    val size: Int get() = owned.size

    fun prepare(pkg: String, tasks: List<TaskPlacement>): Int? {
        if (pkg !in allowed) throw Refusal("app_not_allowed")
        val id = owned[pkg]
        if (id != null) {
            val task = tasks.singleOrNull { it.id == id } ?: throw Refusal("owned_task_missing")
            if (task.display != display || task.basePackage != pkg || task.topPackage != pkg) {
                throw Refusal("target_placement_changed")
            }
        }
        if (tasks.any { it.id != id && (it.basePackage == pkg || it.topPackage == pkg) }) {
            throw Refusal("app_has_unowned_task")
        }
        if (tasks.any { it.display == display && it.id !in ids }) throw Refusal("display_has_unowned_task")
        if (id == null && owned.size >= allowed.size) throw Refusal("session_task_limit")
        return id
    }

    fun admit(pkg: String, before: Set<Int>, task: TaskPlacement) {
        if (pkg !in allowed || owned.containsKey(pkg) || owned.size >= allowed.size ||
            task.id in before || task.id in ids || task.display != display ||
            task.basePackage != pkg || task.topPackage != pkg) throw Refusal("launch_task_not_owned")
        owned[pkg] = task.id
    }
}

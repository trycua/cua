package ai.cua.driver

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test

class SessionTasksTest {
    private val first = "example.first"
    private val second = "example.second"
    private fun placement(id: Int, pkg: String, display: Int = 7) = TaskPlacement(id, display, pkg, pkg)
    private fun refuses(reason: String, block: () -> Unit) {
        assertEquals(reason, assertThrows(Refusal::class.java, block).reason)
    }

    @Test fun twoAppsCanSwitchOnlyToTheirOwnedTasks() {
        val tasks = SessionTasks(setOf(first, second), 7)
        val one = placement(1, first)
        val two = placement(2, second)
        assertNull(tasks.prepare(first, emptyList()))
        tasks.admit(first, emptySet(), one)
        assertNull(tasks.prepare(second, listOf(one)))
        tasks.admit(second, setOf(1), two)
        assertEquals(1, tasks.prepare(first, listOf(two, one)))
        assertEquals(2, tasks.prepare(second, listOf(one, two)))
        assertEquals(setOf(1, 2), tasks.ids)
        refuses("app_not_allowed") { tasks.prepare("example.third", listOf(one, two)) }
    }

    @Test fun existingMainDisplayTaskCannotBeLaunchedOrAdopted() {
        val tasks = SessionTasks(setOf(first), 7)
        refuses("app_has_unowned_task") { tasks.prepare(first, listOf(placement(1, first, 0))) }
        refuses("launch_task_not_owned") { tasks.admit(first, setOf(1), placement(1, first)) }
        refuses("launch_task_not_owned") { tasks.admit(first, emptySet(), placement(1, first, 0)) }
        assertEquals(0, tasks.size)
    }

    @Test fun switchedTaskMustKeepItsIdentityPlacementAndPackage() {
        val tasks = SessionTasks(setOf(first, second), 7)
        tasks.admit(first, emptySet(), placement(1, first))
        refuses("owned_task_missing") { tasks.prepare(first, listOf(placement(2, first))) }
        refuses("target_placement_changed") { tasks.prepare(first, listOf(placement(1, first, 0))) }
        refuses("target_placement_changed") { tasks.prepare(first, listOf(placement(1, second))) }
        refuses("target_placement_changed") {
            tasks.prepare(first, listOf(TaskPlacement(1, 7, first, second)))
        }
        refuses("app_has_unowned_task") {
            tasks.prepare(first, listOf(placement(1, first), placement(2, first, 0)))
        }
    }

    @Test fun unrelatedTaskOnSessionDisplayBlocksLaunch() {
        val tasks = SessionTasks(setOf(first, second), 7)
        refuses("display_has_unowned_task") { tasks.prepare(first, listOf(placement(9, second))) }
        refuses("launch_task_not_owned") { tasks.admit(first, emptySet(), placement(9, second)) }
        assertNull(tasks.prepare(first, listOf(placement(9, second, 0))))
    }
}

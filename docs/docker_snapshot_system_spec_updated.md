# **Docker Container Snapshot System Implementation Specification**

## **Overview**

Implement a snapshot-based state management system for the CUA Agent SDK that captures and manages Docker container states at each agent turn. This system will provide the foundation for state persistence, debugging, and potential future rollback capabilities using Docker's checkpoint functionality.

## **Objective**

Create a snapshot management system that:

* Captures Docker container snapshots at configurable intervals during agent execution  
* Stores and manages snapshot history with metadata (timestamp, action context, screenshots)  
* Provides APIs to restore container state from any saved snapshot  
* Maintains efficient storage with compression and automatic cleanup  
* Integrates seamlessly with the existing Agent SDK callback architecture

## **Implementation Approach**

Implement snapshot functionality at the VM provider level with integration through the Computer class and callback-based automation:

### **Base VM Provider Interface (base.py)**

Add abstract snapshot methods to the BaseVMProvider class:

```python
@abc.abstractmethod
async def create_snapshot(self, name: str, snapshot_id: str, metadata: Optional[Dict[str, Any]] = None) -> bool:
    """Create a snapshot of the specified VM/container.
    
    Args:
        name: Name of the VM/container to snapshot
        snapshot_id: Unique identifier for the snapshot
        metadata: Optional metadata to store with the snapshot
        
    Returns:
        True if snapshot was created successfully, False otherwise
    """
    pass

@abc.abstractmethod
async def restore_snapshot(self, name: str, snapshot_id: str) -> bool:
    """Restore a VM/container from a snapshot.
    
    Args:
        name: Name of the VM/container to restore
        snapshot_id: Unique identifier of the snapshot to restore from
        
    Returns:
        True if restore was successful, False otherwise
    """
    pass

@abc.abstractmethod
async def list_snapshots(self, name: str) -> List[SnapshotInfo]:
    """List all available snapshots for a VM/container.
    
    Args:
        name: Name of the VM/container to list snapshots for
        
    Returns:
        List of SnapshotInfo objects containing snapshot metadata
    """
    pass
```

### **Snapshot Callback Handler**

Implement as a callback in the Agent SDK's existing callback architecture:

```python
class DockerSnapshotCallback(AsyncCallbackHandler):
    def __init__(self, computer: Computer, snapshot_config: Dict[str, Any]):
        """
        Initialize snapshot callback with configuration.
        
        Args:
            computer: Computer instance with Docker provider
            snapshot_config: Configuration dict with:
                - snapshot_interval: "every_run_start" | "every_action" | "every_run_end"
                - max_snapshots: Maximum number to retain
                - auto_cleanup: Whether to cleanup old snapshots
                - metadata_capture: What metadata to include
        """
        # Initialize snapshot management
        # Store computer reference and config
        # Setup snapshot storage and cleanup policies
        pass
    
    async def on_run_start(self, kwargs, old_items):
        """Create snapshot after run starts if configured."""
        # Check if snapshot_config["snapshot_interval"] == "every_run_start"
        # Generate snapshot ID with timestamp
        # Call computer.create_snapshot() with metadata
        # Log snapshot creation
        pass
    
    async def on_responses(self, kwargs, responses):
        """Create snapshot after every action if configured."""
        # Check if snapshot_config["snapshot_interval"] == "every_action"
        # Generate snapshot ID with action context
        # Call computer.create_snapshot() with action metadata
        # Manage snapshot retention (cleanup old ones)
        pass
    
    async def on_run_end(self, kwargs, old_items, new_items):
        """Create snapshot after run ends if configured."""
        # Check if snapshot_config["snapshot_interval"] == "every_run_end"
        # Generate final snapshot ID
        # Call computer.create_snapshot() with run summary
        # Perform cleanup if configured
        pass
```

### **Docker Provider Implementation (docker/provider.py)**

Implement snapshot functionality using Docker checkpoint/restore:

```python
async def create_snapshot(self, name: str, snapshot_id: str, metadata: Optional[Dict[str, Any]] = None) -> bool:
    """Create a Docker checkpoint snapshot."""
    try:
        # 1. Create Docker checkpoint
        cmd = ["docker", "checkpoint", "create", name, snapshot_id]
        result = subprocess.run(cmd, capture_output=True, text=True, check=True)
        
        # 2. Store metadata if provided
        if metadata:
            await self._save_snapshot_metadata(snapshot_id, metadata)
            
        return True
    except subprocess.CalledProcessError as e:
        logger.error(f"Failed to create snapshot {snapshot_id}: {e.stderr}")
        return False

async def restore_snapshot(self, name: str, snapshot_id: str) -> bool:
    """Restore Docker container from checkpoint."""
    try:
        # 1. Stop current container
        await self.stop_vm(name)
        
        # 2. Start from checkpoint
        cmd = ["docker", "start", "--checkpoint", snapshot_id, name]
        result = subprocess.run(cmd, capture_output=True, text=True, check=True)
        
        return True
    except subprocess.CalledProcessError as e:
        logger.error(f"Failed to restore snapshot {snapshot_id}: {e.stderr}")
        return False

async def list_snapshots(self, name: str) -> List[SnapshotInfo]:
    """List available Docker checkpoints."""
    try:
        # List checkpoints for container
        cmd = ["docker", "checkpoint", "ls", name]
        result = subprocess.run(cmd, capture_output=True, text=True, check=True)
        
        # Parse and return snapshot info
        return await self._parse_checkpoint_list(result.stdout)
    except subprocess.CalledProcessError as e:
        logger.error(f"Failed to list snapshots: {e.stderr}")
        return []
```

### **Other VM Providers (NotImplementedError)**

All other VM providers (Lume, Lumier, Cloud, WinSandbox) will raise NotImplementedError:

```python
async def create_snapshot(self, name: str, snapshot_id: str, metadata: Optional[Dict[str, Any]] = None) -> bool:
    raise NotImplementedError("Snapshot functionality not implemented for this provider")

async def restore_snapshot(self, name: str, snapshot_id: str) -> bool:
    raise NotImplementedError("Snapshot functionality not implemented for this provider")

async def list_snapshots(self, name: str) -> List[SnapshotInfo]:
    raise NotImplementedError("Snapshot functionality not implemented for this provider")
```

### **Computer Class Integration (computer.py)**

Add snapshot methods to the Computer class that delegate to the VM provider:

```python
async def create_snapshot(self, snapshot_id: str) -> bool:
    """Create a snapshot of the current VM/container.
    
    Args:
        snapshot_id: Unique identifier for the snapshot
        
    Returns:
        True if snapshot was created successfully, False otherwise
    """
    if not self.config.vm_provider:
        raise RuntimeError("No VM provider available for snapshot operations")
    
    # Use the current container/VM name
    vm_name = self.name or "default"
    
    # Capture current screenshot and metadata
    metadata = {
        "timestamp": datetime.now().isoformat(),
        "display": {"width": self.display.width, "height": self.display.height},
    }
    
    try:
        # Capture screenshot if interface is available
        if hasattr(self, '_interface') and self._interface:
            screenshot = await self.interface.screenshot()
            metadata["screenshot_size"] = len(screenshot)
    except Exception as e:
        logger.warning(f"Failed to capture screenshot for snapshot: {e}")
    
    return await self.config.vm_provider.create_snapshot(vm_name, snapshot_id, metadata)

async def restore_snapshot(self, snapshot_id: str) -> bool:
    """Restore the current VM/container from a snapshot.
    
    Args:
        snapshot_id: Unique identifier of the snapshot to restore from
        
    Returns:
        True if restore was successful, False otherwise
    """
    if not self.config.vm_provider:
        raise RuntimeError("No VM provider available for snapshot operations")
    
    vm_name = self.name or "default"
    
    # Restore the snapshot
    success = await self.config.vm_provider.restore_snapshot(vm_name, snapshot_id)
    
    if success:
        # Re-establish interface connection after restore
        try:
            await self._reconnect_interface()
        except Exception as e:
            logger.error(f"Failed to reconnect interface after snapshot restore: {e}")
            return False
    
    return success

async def list_snapshots(self) -> List[SnapshotInfo]:
    """List all available snapshots for the current VM/container.
    
    Returns:
        List of SnapshotInfo objects containing snapshot metadata
    """
    if not self.config.vm_provider:
        raise RuntimeError("No VM provider available for snapshot operations")
    
    vm_name = self.name or "default"
    return await self.config.vm_provider.list_snapshots(vm_name)

async def _reconnect_interface(self):
    """Re-establish interface connection after snapshot restore."""
    if hasattr(self, '_interface') and self._interface:
        # Disconnect current interface
        await self.disconnect()
        
        # Wait for container to be ready
        await self.wait_vm_ready()
        
        # Reconnect interface
        self._interface = await InterfaceFactory.create_interface(
            self.config.interface_type,
            self.get_ip(),
            self.config.interface_port,
            self.config.interface_host,
            self.config.interface_api_key,
            self.config.interface_container_name,
            self.config.interface_os_type,
        )
```

## **Technical Requirements**

### **Container State Management**

* **Docker Checkpoint/Restore (CRIU)**:  
  * Utilize Docker's experimental checkpoint feature for container state capture  
  * Configure Docker daemon with --experimental flag  
  * Implement wrapper around docker checkpoint create and docker start --checkpoint  
* **Snapshot Storage**:  
  * Store snapshots in dedicated volume mount  
  * Implement compression for space efficiency (gzip/lz4)  
  * Automatic cleanup of old snapshots based on configurable retention policy

### **Snapshot Management Features**

```python
@dataclass
class SnapshotInfo:
    snapshot_id: str
    timestamp: datetime
    metadata: Dict[str, Any]  # Action context, screenshot info, etc.
    size_bytes: int
    container_name: str
    
    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary for storage/serialization"""
        return asdict(self)

class SnapshotManager:
    async def create_snapshot(self, container_name: str, snapshot_id: str, metadata: Dict[str, Any]) -> str:
        # 1. Generate unique snapshot ID if not provided
        if not snapshot_id:
            snapshot_id = f"snapshot_{datetime.now().isoformat()}"
          
        # 2. Create Docker checkpoint
        await self._docker_checkpoint(container_name, snapshot_id)
          
        # 3. Store metadata
        self.save_snapshot_metadata(snapshot_id, metadata)
          
        return snapshot_id
      
    async def restore_snapshot(self, container_name: str, snapshot_id: str) -> bool:
        # 1. Stop current container
        await self._stop_container(container_name)
          
        # 2. Restore from checkpoint
        await self._docker_restore(container_name, snapshot_id)
          
        return True
      
    async def delete_snapshot(self, snapshot_id: str) -> bool:
        # Remove snapshot files and metadata
        return await self._cleanup_snapshot(snapshot_id)
```

## **Core Processing Pipeline**

### **Snapshot Creation:**

* Create Docker checkpoint of current container state  
* Capture screenshot and action metadata  
* Store snapshot with unique ID and timestamp

### **Snapshot Restoration:**

* Stop current container gracefully
* Restore from specified checkpoint  
* Re-establish interface connections
* Resume execution from restored state

### **Snapshot Management:**

* List available snapshots with metadata  
* Delete old snapshots based on retention policy
* Compression and efficient storage

### **Snapshot Storage Structure**

```python
@dataclass  
class SnapshotInfo:  
    snapshot_id: str  
    timestamp: datetime  
    container_name: str
    metadata: Dict[str, Any]  # Screenshot info, action context, etc.
    checkpoint_path: str  
    size_bytes: int  
      
    def to_dict(self) -> Dict[str, Any]:  
        """Convert to dictionary for storage/serialization"""  
        return asdict(self)
```

## **Integration Points**

### **Computer SDK Integration**

* Extend DockerProvider with checkpoint/restore capabilities  
* Add snapshot management methods to Computer class  
* Implement async handlers for state persistence

### **Agent SDK Integration**

* Use Computer class snapshot methods in callbacks
* Hook into existing callback lifecycle methods  
* Maintain compatibility with other callbacks

## **Configuration**

```python
# Snapshot only at run start
run_start_config = {
    "snapshot_interval": "every_run_start",
    "max_snapshots": 3,
    "auto_cleanup": True
}

# Snapshot after every action
every_action_config = {
    "snapshot_interval": "every_action",
    "max_snapshots": 20,
    "auto_cleanup": True,
    "metadata_capture": {
        "screenshot": True,
        "action_type": True,
        "coordinates": True
    }
}

# Snapshot only at run end
run_end_config = {
    "snapshot_interval": "every_run_end",
    "max_snapshots": 5,
    "auto_cleanup": False,  # Keep all end snapshots
    "metadata_capture": {
        "final_state": True,
        "execution_summary": True
    }
}
```

## **Resources**

* **Docker Checkpoint/Restore**: [https://docs.docker.com/engine/reference/commandline/checkpoint/](https://docs.docker.com/engine/reference/commandline/checkpoint/)  
* **CRIU (Checkpoint/Restore In Userspace)**: [https://criu.org/](https://criu.org/)  
* **Computer SDK Docker Provider**: libs/python/computer/computer/providers/docker.py  
* **Agent Callback System**: libs/python/agent/agent/callbacks/base.py  
* **Trajectory Saver Reference**: libs/python/agent/agent/callbacks/trajectory_saver.py

## **Success Criteria**

* Successfully captures and stores Docker container snapshots at configurable intervals  
* Provides reliable snapshot restoration functionality on demand  
* Maintains bounded snapshot history with automatic cleanup  
* Stores comprehensive metadata with each snapshot (screenshots, action context, timestamps)  
* Integrates with existing Computer SDK without breaking changes  
* Handles edge cases (network failures, storage limits, concurrent operations)  
* Provides clear logging and debugging information  
* Performance overhead < 5% for typical agent operations

## **Edge Cases and Considerations**

1. **Storage Management**:  
   * Handle disk space limitations gracefully  
   * Implement snapshot pruning strategies  
   * Support external storage backends (S3, NFS)  
2. **Network State**:  
   * Preserve network connections across rollbacks  
   * Handle WebSocket reconnection  
   * Maintain session state  
3. **Concurrent Operations**:  
   * Thread-safe snapshot operations  
   * Handle multiple agents on same container  
   * Coordinate rollback decisions  
4. **Recovery Scenarios**:  
   * Corrupted snapshots  
   * Failed rollback attempts  
   * Cascading failures

## **Example Usage**

```python
import asyncio  
from agent import ComputerAgent  
from computer import Computer

async def main():  
    # Initialize Computer with Docker provider
    async with Computer(  
        provider_type="docker",  
        image="cua-ubuntu:latest",  
        name="my-container"
    ) as computer:  
          
        # Create manual snapshot
        snapshot_id = "before_navigation"
        success = await computer.create_snapshot(snapshot_id)
        print(f"Snapshot created: {success}")
        
        # Perform some actions...
        await computer.interface.click(100, 200)
        
        # List available snapshots
        snapshots = await computer.list_snapshots()
        print(f"Available snapshots: {[s.snapshot_id for s in snapshots]}")
        
        # Restore to previous snapshot if needed
        if snapshots:
            restored = await computer.restore_snapshot(snapshot_id)
            print(f"Restored to snapshot: {restored}")

if __name__ == "__main__":  
    asyncio.run(main())
```

## **Target Timeline (1 Week)**

* **Day 1-2**: Add abstract snapshot methods to BaseVMProvider and implement NotImplementedError in other providers
* **Day 3-4**: Implement Docker checkpoint/restore functionality in DockerProvider  
* **Day 5-6**: Add snapshot methods to Computer class with proper interface reconnection
* **Day 7**: Testing, optimization, and documentation

## **Deliverables**

1. **BaseVMProvider** extensions with abstract snapshot methods
2. **DockerProvider** implementation with checkpoint/restore operations  
3. **Computer** class integration with snapshot management methods
4. **SnapshotManager** class for managing snapshot lifecycle and storage  
5. Comprehensive documentation and API reference  
6. Unit and integration tests for snapshot operations  
7. Example scripts demonstrating snapshot usage and restoration  
8. Performance benchmarks showing overhead of snapshot operations

## **Future Extensions**

Once the core snapshot system is implemented and tested, the following features can be added:

* **Trajectory Validation**: Add deviation detection mechanisms  
* **Automatic Rollback**: Implement automatic rollback on detected deviations  
* **Visual Comparison**: Add screenshot similarity scoring for validation  
* **Pattern Detection**: Identify action loops and repeated failures  
* **Smart Rollback Selection**: Choose optimal snapshot for restoration based on context

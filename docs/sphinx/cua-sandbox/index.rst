CUA Sandbox API
===============

This generated comparison artifact is built by Sphinx from the public Python
SDK. Fumadocs does not process these RST directives; it links to the generated
HTML artifact.

Public exports
--------------

.. autodata:: cua_sandbox.__all__

.. include:: public-exports.rst

Configuration
-------------

.. autofunction:: cua_sandbox.configure

Sandbox lifecycle
-----------------

.. autoclass:: cua_sandbox.Sandbox
   :members: create, delete, disconnect

.. automethod:: cua_sandbox.Sandbox.__aenter__

.. automethod:: cua_sandbox.Sandbox.__aexit__

.. autoclass:: cua_sandbox.SandboxInfo
   :members:

.. autofunction:: cua_sandbox.sandbox

.. autoclass:: cua_sandbox.Localhost
   :members: disconnect

.. autofunction:: cua_sandbox.localhost

Image builder
-------------

.. autoclass:: cua_sandbox.Image
   :members:

Runtime support
---------------

.. autoclass:: cua_sandbox.RuntimeSupport

.. autofunction:: cua_sandbox.check_local_support

.. autofunction:: cua_sandbox.skip_if_unsupported

Exported transport
------------------

.. autoclass:: cua_sandbox.CloudTransport

Public interfaces
-----------------

.. autoclass:: cua_sandbox.interfaces.Apps
   :members:

.. autoclass:: cua_sandbox.interfaces.Clipboard
   :members:

.. autoclass:: cua_sandbox.interfaces.FileEntry

.. autoclass:: cua_sandbox.interfaces.Files
   :members:

.. autoclass:: cua_sandbox.interfaces.Keyboard
   :members:

.. autoclass:: cua_sandbox.interfaces.Mobile
   :members:

.. autoclass:: cua_sandbox.interfaces.Mouse
   :members:

.. autoclass:: cua_sandbox.interfaces.Screen
   :members:

.. autoclass:: cua_sandbox.interfaces.Shell
   :members:

.. autoclass:: cua_sandbox.interfaces.Terminal
   :members:

.. autoclass:: cua_sandbox.interfaces.Tunnel
   :members:

.. autoclass:: cua_sandbox.interfaces.TunnelInfo

.. autoclass:: cua_sandbox.interfaces.Window
   :members:

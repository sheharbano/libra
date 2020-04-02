# Follow the instructions in `readme.md` to get started.
# Remember to call `source ~/bin/aws-mfa` every 24 hours.
import boto3
from botocore.exceptions import ClientError
from fabric import task, Connection, ThreadingGroup as Group
from paramiko import RSAKey
import os

ec2 = boto3.client('ec2')
region = os.environ.get("AWS_EC2_REGION")


# --- Start Config ---

def credentials():
    ''' Set the username and path to key file. '''
    return {
        'user': 'ubuntu',
        'keyfile': '/Users/asonnino/.ssh/aws-fb.pem'
    }


def filter(instance):
    ''' Specify a filter to select only the desired hosts. '''
    name = next(tag['Value'] for tag in instance.tags if 'Name'in tag['Key'])
    return 'twins'.casefold() in name.casefold()

# --- End Config ---


def set_hosts(ctx, status='running', cred=credentials, filter=filter):
    ''' Helper function to set the credentials and a list of instances into
    context; the instances are filtered with the filter provided as input.
    '''
    # Set credentials into the context
    credentials = cred()
    ctx.user = credentials['user']
    ctx.keyfile = credentials['keyfile']  # This is only used the task `info`
    ctx.connect_kwargs.pkey = RSAKey.from_private_key_file(
        credentials['keyfile'])

    # Get all instances for a given status.
    ec2resource = boto3.resource('ec2')
    instances = ec2resource.instances.filter(
        Filters=[{'Name': 'instance-state-name', 'Values': [status]}]
    )

    # Get all instances that match the input filter
    ctx.instances = [x for x in instances if filter(x)]
    ctx.hosts = [x.public_ip_address for x in instances if filter(x)]


@task
def test(ctx):
    ''' Test the connection with all hosts. If the command succeeds, it
    prints "Hello, World!" for each host.

    COMMANDS:	fab test
    '''
    set_hosts(ctx)
    g = Group(*ctx.hosts, user=ctx.user, connect_kwargs=ctx.connect_kwargs)
    g.run('echo "Hello, World!"')


@task
def info(ctx):
    ''' Print commands to ssh into hosts (debug).

    COMMANDS:	fab info
    '''
    set_hosts(ctx)
    print('\nAvailable machines:')
    for host in ctx.hosts:
        print(f'\t ssh -i {ctx.keyfile} {ctx.user}@{host}')
    print()


@task
def start(ctx):
    ''' Start instances.

    COMMANDS:	fab start
    '''
    set_hosts(ctx, status='stopped')
    ids = [instance.id for instance in ctx.instances]
    if not ids:
        print('There are no instances to start.')
        return
    response = ec2.start_instances(InstanceIds=ids, DryRun=False)
    print(response['StartingInstances'])


@task
def stop(ctx):
    ''' Stop instances.

    COMMANDS:	fab stop
    '''
    set_hosts(ctx, status='running')
    ids = [instance.id for instance in ctx.instances]
    response = ec2.stop_instances(InstanceIds=ids, DryRun=False)
    print(response)


@task
def install(ctx):
    ''' Cleanup and install twins on a fresh machine.

    COMMANDS:	fab install
    '''
    script = 'twins-aws-setup.sh'

    set_hosts(ctx)
    for host in ctx.hosts:
        c = Connection(host, user=ctx.user, connect_kwargs=ctx.connect_kwargs)
        c.put(script, '.')
        c.run(f'chmod +x {script}')

        # TODO: find a way to forgo the grub config prompt and run the setup
        # script automatically.
    print(f'The script "{script}"" is now uploaded on every machine;'
          'Run it manually and pay attention to the APT grub config prompt.')


@task
def update(ctx):
    ''' Update the software from Github.

    COMMANDS:	fab update
    '''
    set_hosts(ctx)
    g = Group(*ctx.hosts, user=ctx.user, connect_kwargs=ctx.connect_kwargs)
    g.run('cd libra/ && git pull')


@task
def run(ctx):
    ''' Runs experiments with the specified configs.

    COMMANDS:	fab run
    '''
    script = 'twins-aws-run.sh'
    runs = '5'

    set_hosts(ctx)

    # Upload / update script
    for host in ctx.hosts:
        c = Connection(host, user=ctx.user, connect_kwargs=ctx.connect_kwargs)
        c.put(script, '.')
        c.run(f'chmod +x {script}')

    # Run script on all machines in parallel
    g = Group(*ctx.hosts, user=ctx.user, connect_kwargs=ctx.connect_kwargs)
    g.run(f'screen -d -m ./{script} {runs}')
